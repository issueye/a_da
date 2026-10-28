use std::io::{Cursor, Write};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use plugin_hub::config::Config;
use plugin_hub::db;
use plugin_hub::storage::Storage;
use plugin_hub::{AppState, app};
use tempfile::TempDir;
use tower::ServiceExt;

struct Ctx {
    app: Router,
    _dir: TempDir,
}

async fn setup() -> Ctx {
    let dir = tempfile::tempdir().expect("临时目录");
    let pool = db::test_pool().await;
    let storage = Storage::new(dir.path().join("assets")).expect("资产目录");
    let config = Config::for_tests(dir.path().join("assets"));
    let state = AppState::new(pool, storage, config);
    Ctx {
        app: app(state),
        _dir: dir,
    }
}

async fn body_text(resp: Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

fn set_cookie(resp: &Response<Body>) -> String {
    resp.headers()
        .get(header::SET_COOKIE)
        .expect("应有 Set-Cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn extract_csrf(html: &str) -> String {
    let marker = "name=\"_csrf\" value=\"";
    let start = html.find(marker).expect("页面应有 CSRF 隐藏字段") + marker.len();
    let end = html[start..].find('"').expect("CSRF 值应有结束引号") + start;
    html[start..end].to_string()
}

async fn register(app: &Router, username: &str, email: &str) -> (String, String) {
    let form =
        format!("username={username}&email={email}&password=password123&password2=password123");
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "注册应成功");
    let cookie = set_cookie(&resp);

    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/new")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let csrf = extract_csrf(&body_text(page).await);
    (cookie, csrf)
}

async fn create_plugin(app: &Router, cookie: &str, csrf: &str, slug: &str) -> Response<Body> {
    let form = format!(
        "_csrf={csrf}&slug={slug}&name=Hello+Plugin&description=demo&readme=%23+Hello%0A%0AThis+is+demo.&tags=git%2Creview&homepage=&license=MIT"
    );
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn plugin_source(version: &str) -> String {
    format!(
        r#"const metadata = {{
  name: 'demo',
  description: '演示插件',
  author: 'alice',
  version: '{version}',
  tools: {{ count: 0, names: [] }},
  systemPrompt: {{ count: 0, names: [] }},
  skills: {{ count: 0, names: [] }},
  events: {{ count: 0, names: [] }}
}};
export default function (ada) {{ ada.metadata = metadata; }}"#
    )
}

fn zip_bytes_for(source: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("index.ts", opts).unwrap();
        zip.write_all(source.as_bytes()).unwrap();
        zip.start_file("README.md", opts).unwrap();
        zip.write_all("# 包内 README\n\n来自插件包的 README 内容。".as_bytes())
            .unwrap();
        zip.finish().unwrap();
    }
    buf.into_inner()
}

fn zip_bytes() -> Vec<u8> {
    zip_bytes_for(&plugin_source("0.1.0"))
}

async fn upload_release(
    app: &Router,
    cookie: &str,
    csrf: &str,
    owner: &str,
    slug: &str,
    version: &str,
) -> Response<Body> {
    upload_release_asset(
        app,
        cookie,
        csrf,
        owner,
        slug,
        &zip_bytes_for(&plugin_source(version)),
        version,
    )
    .await
}

/// 上传指定版本号与指定包内容（用于元数据/版本不一致的负向用例）。
async fn upload_release_asset(
    app: &Router,
    cookie: &str,
    csrf: &str,
    owner: &str,
    slug: &str,
    bytes: &[u8],
    form_version: &str,
) -> Response<Body> {
    let boundary = "----ada_plugin_hub_test";
    let mut body = Vec::new();
    for (name, value) in [
        ("_csrf", csrf),
        ("version", form_version),
        ("notes", "first release"),
    ] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"asset\"; filename=\"plugin.zip\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/plugins/{owner}/{slug}/releases/new"))
                .header(header::COOKIE, cookie)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn register_create_and_search() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "alice", "alice@example.com").await;
    let resp = create_plugin(&ctx.app, &cookie, &csrf, "hello").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/plugins/alice/hello?ok=created"
    );

    let page = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/alice/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(html.contains("Hello Plugin"));
    assert!(html.contains("This is demo."));
    assert!(html.contains("@alice"));

    let search = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=Hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(search).await.contains("Hello Plugin"));

    let health = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

#[tokio::test]
async fn release_upload_download_and_star() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "bob", "bob@example.com").await;
    let created = create_plugin(&ctx.app, &cookie, &csrf, "tool").await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);

    let upload = upload_release(&ctx.app, &cookie, &csrf, "bob", "tool", "0.1.0").await;
    assert_eq!(upload.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        upload.headers().get(header::LOCATION).unwrap(),
        "/plugins/bob/tool?ok=released"
    );

    let detail = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/bob/tool")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_text(detail).await;
    assert!(html.contains("v0.1.0"));
    assert!(html.contains("first release"));
    // 发布时从包根 index.ts 提取的元数据会显示在版本卡上
    assert!(html.contains("包内 v0.1.0"));
    assert!(html.contains("alice"));
    // 插件包同时同步了仓库简介（metadata.description）与 README.md
    assert!(html.contains("演示插件"), "{html}");
    assert!(html.contains("来自插件包的 README 内容"), "{html}");

    let download = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/bob/tool/releases/0.1.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        download.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/zip"
    );
    let bytes = download.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], &zip_bytes()[..]);

    let star = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins/bob/tool/star")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(star.status(), StatusCode::OK);
    let star_html = body_text(star).await;
    assert!(star_html.contains("已 Star"));
    assert!(star_html.contains(">1<"));
}

#[tokio::test]
async fn release_requires_package_metadata() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "meta", "meta@example.com").await;
    let created = create_plugin(&ctx.app, &cookie, &csrf, "meta-plugin").await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);

    // 1) 缺 author：必须在发布时被拒，并把原因回显到表单
    let missing_author = zip_bytes_for(
        r#"export default function (ada) {
            ada.metadata = { name: 'meta-plugin', description: '缺作者', version: '0.1.0' };
        }"#,
    );
    let resp = upload_release_asset(
        &ctx.app,
        &cookie,
        &csrf,
        "meta",
        "meta-plugin",
        &missing_author,
        "0.1.0",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.contains("author"));

    // 2) 完全没有 metadata：提示要写 ada.metadata
    let no_metadata = zip_bytes_for("export default function (ada) {}");
    let resp = upload_release_asset(
        &ctx.app,
        &cookie,
        &csrf,
        "meta",
        "meta-plugin",
        &no_metadata,
        "0.1.0",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.contains("metadata"));

    // 3) 表单版本与包内版本不一致：拒绝，并说明两边各是什么
    let mismatched = zip_bytes_for(&plugin_source("0.2.0"));
    let resp = upload_release_asset(
        &ctx.app,
        &cookie,
        &csrf,
        "meta",
        "meta-plugin",
        &mismatched,
        "0.3.0",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("不一致"), "{html}");

    // 4) 表单版本留空：采用包内 metadata.version
    let resp = upload_release_asset(
        &ctx.app,
        &cookie,
        &csrf,
        "meta",
        "meta-plugin",
        &zip_bytes_for(&plugin_source("1.2.3")),
        "",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let detail = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/meta/meta-plugin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(detail).await.contains("v1.2.3"));
}

#[tokio::test]
async fn api_me_and_upsert_support_desktop_publishing() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "publisher", "publisher@example.com").await;

    let token_page = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/tokens")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "_csrf={csrf}&name=publish&read=1&write=1"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(token_page.status(), StatusCode::OK);
    let token = extract_api_token(&body_text(token_page).await);
    let auth = format!("Bearer {token}");

    let me = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let me_json: serde_json::Value =
        serde_json::from_str(&body_text(me).await).expect("me 应是 JSON");
    assert_eq!(me_json["username"], "publisher");
    assert_eq!(me_json["canWrite"], true);

    let create_body = serde_json::json!({
        "slug": "desktop-publish",
        "name": "Desktop Publish",
        "description": "从桌面端上传",
        "readme": "# v1",
        "tags": "desktop,upload"
    });
    let created = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/plugins?upsert=1")
                .header(header::AUTHORIZATION, &auth)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_json: serde_json::Value =
        serde_json::from_str(&body_text(created).await).unwrap();
    assert_eq!(created_json["existing"], false);
    assert_eq!(created_json["owner"], "publisher");

    // 第二次 upsert：返回 200 + existing，并更新元数据（同一只 Token 可重复发布）
    let updated_body = serde_json::json!({
        "slug": "desktop-publish",
        "name": "Desktop Publish v2",
        "description": "更新后的简介",
        "readme": "# v2",
        "tags": "desktop"
    });
    let updated = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/plugins?upsert=1")
                .header(header::AUTHORIZATION, &auth)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(updated_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_json: serde_json::Value =
        serde_json::from_str(&body_text(updated).await).unwrap();
    assert_eq!(updated_json["existing"], true);

    let detail = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins/publisher/desktop-publish")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail_json: serde_json::Value =
        serde_json::from_str(&body_text(detail).await).unwrap();
    assert_eq!(detail_json["name"], "Desktop Publish v2");
    assert_eq!(detail_json["description"], "更新后的简介");

    // 不带 upsert 的重复创建必须明确 409（而不是 500）
    let duplicate = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/plugins")
                .header(header::AUTHORIZATION, &auth)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn market_lists_latest_release_and_metadata() {    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "market", "market@example.com").await;
    assert_eq!(
        create_plugin(&ctx.app, &cookie, &csrf, "market-plugin")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        upload_release(&ctx.app, &cookie, &csrf, "market", "market-plugin", "0.1.0")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );

    // 市场 API 与普通 API 一样需要 read 作用域
    let token_page = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/tokens")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}&name=market&read=1")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(token_page.status(), StatusCode::OK);
    let token = extract_api_token(&body_text(token_page).await);

    let resp = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/market/plugins?q=market")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_str(&body_text(resp).await).expect("市场响应应是 JSON");
    let plugin = &body["plugins"][0];
    assert_eq!(plugin["slug"], "market-plugin");
    assert_eq!(plugin["version"], "0.1.0");
    assert_eq!(plugin["has_release"], true);
    assert_eq!(plugin["metadata"]["author"], "alice");
    assert_eq!(plugin["metadata"]["version"], "0.1.0");

    let anonymous = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/market/plugins")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn edit_form_can_sync_description_and_readme_from_latest_package() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "syncer", "syncer@example.com").await;
    assert_eq!(
        create_plugin(&ctx.app, &cookie, &csrf, "sync-me")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        upload_release(&ctx.app, &cookie, &csrf, "syncer", "sync-me", "0.1.0")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );

    // 人工改成别的简介/README
    let edit_page = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/syncer/sync-me/edit")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let edit_csrf = extract_csrf(&body_text(edit_page).await);
    let manual = format!(
        "_csrf={edit_csrf}&name=Sync+Me&description=manual-desc&readme=manual-readme&homepage=&license=MIT&tags=demo"
    );
    let updated = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins/syncer/sync-me")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(manual))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::SEE_OTHER);

    // 从最新版本插件包同步：简介取 metadata.description，README 取包根 README.md
    let synced = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins/syncer/sync-me/sync-package")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={edit_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(synced.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        synced.headers().get(header::LOCATION).unwrap(),
        "/plugins/syncer/sync-me/edit?ok=synced"
    );

    let detail = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/syncer/sync-me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_text(detail).await;
    assert!(html.contains("演示插件"), "{html}");
    assert!(html.contains("来自插件包的 README 内容"), "{html}");
    assert!(!html.contains("manual-desc"), "{html}");

    // 没有发布版本的插件：同步按钮要给出可操作提示，而不是 500
    assert_eq!(
        create_plugin(&ctx.app, &cookie, &csrf, "no-release")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let no_release_page = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/syncer/no-release/edit")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let no_release_csrf = extract_csrf(&body_text(no_release_page).await);
    let no_release = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins/syncer/no-release/sync-package")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={no_release_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_release.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        no_release.headers().get(header::LOCATION).unwrap(),
        "/plugins/syncer/no-release/edit?err=no_release"
    );
}

#[tokio::test]
async fn anonymous_is_redirected_to_login() {
    let ctx = setup().await;
    let resp = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");

    let star = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins/nobody/nothing/star")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(star.status(), StatusCode::SEE_OTHER);
    assert!(
        star.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("/login?next=")
    );
}

#[tokio::test]
async fn invalid_slug_is_rejected_with_form_error() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "carol", "carol@example.com").await;
    let resp = create_plugin(&ctx.app, &cookie, &csrf, "Bad Slug").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("slug"));
}

#[tokio::test]
async fn login_rejects_wrong_password_and_accepts_correct() {
    let ctx = setup().await;
    let _ = register(&ctx.app, "dave", "dave@example.com").await;

    let wrong = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("login=dave&password=wrong-password"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::OK);
    assert!(body_text(wrong).await.contains("用户名或密码错误"));

    let ok = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("login=dave&password=password123&next=/plugins"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::SEE_OTHER);
    assert_eq!(ok.headers().get(header::LOCATION).unwrap(), "/plugins");
    assert!(ok.headers().get(header::SET_COOKIE).is_some());
}

#[tokio::test]
async fn owner_can_edit_plugin_without_resubmitting_slug() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "erin", "erin@example.com").await;
    assert_eq!(
        create_plugin(&ctx.app, &cookie, &csrf, "edit-me")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );

    let edit = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/erin/edit-me/edit")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);
    let edit_csrf = extract_csrf(&body_text(edit).await);

    // 编辑表单不提交 slug：服务端必须允许缺省，并以路径里的 slug 为准。
    let form = format!(
        "_csrf={edit_csrf}&name=Renamed+Plugin&description=updated&readme=%23+Updated&homepage=&license=MIT&tags=demo"
    );
    let resp = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins/erin/edit-me")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/plugins/erin/edit-me?ok=updated"
    );

    let detail = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/erin/edit-me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(detail).await.contains("Renamed Plugin"));
}

#[tokio::test]
async fn static_assets_are_served_locally() {
    let ctx = setup().await;
    let css = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/static/app.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
    assert!(
        css.headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    let css_text = body_text(css).await;
    // 组件层由 Tailwind 从 `static/input.css` 生成；`.card` 是设计系统的稳定入口，
    // 断言它而不是某个一次性的调色板类，避免调色板升级就误报。
    assert!(css_text.contains(".card{") || css_text.contains(".card {"));

    for (path, marker) in [
        ("/static/theme.js", "a_da_theme"),
        ("/static/app.js", "data-confirm-dialog"),
    ] {
        let asset = ctx
            .app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(asset.status(), StatusCode::OK, "{path} 应可访问");
        assert!(
            asset
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/javascript"),
            "{path} 的 Content-Type 应为 JS"
        );
        assert!(body_text(asset).await.contains(marker), "{path} 内容不符");
    }

    let htmx = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/static/htmx.min.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(htmx.status(), StatusCode::OK);
    assert!(body_text(htmx).await.contains("htmx"));

    let home = ctx
        .app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let html = body_text(home).await;
    assert!(html.contains("/static/app.css"));
    assert!(html.contains("/static/app.js"));
    assert!(html.contains("/static/theme.js"));
    assert!(html.contains("/static/htmx.min.js"));
    assert!(html.contains("/static/alpine.min.js"));
    assert!(!html.contains("cdn.tailwindcss.com"));
}

#[tokio::test]
async fn org_create_add_member_and_publish_org_plugin() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "alice", "alice@example.com").await;
    let _ = register(&ctx.app, "bob", "bob@example.com").await;

    let create_org = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "_csrf={csrf}&name=acme&display_name=Acme&description=Acme+plugins"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_org.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        create_org.headers().get(header::LOCATION).unwrap(),
        "/orgs/acme?ok=created"
    );

    let add = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/acme/members")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}&username=bob&role=member")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(add.status(), StatusCode::SEE_OTHER);

    let team = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/acme/teams")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "_csrf={csrf}&name=core&description=Core+team"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(team.status(), StatusCode::SEE_OTHER);

    let org_page = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/orgs/acme")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(org_page.status(), StatusCode::OK);
    let org_html = body_text(org_page).await;
    assert!(org_html.contains("Acme"));
    assert!(org_html.contains("@bob"));
    assert!(org_html.contains("core"));

    let created = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/plugins")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "_csrf={csrf}&slug=org-plugin&name=Org+Plugin&description=org+demo&readme=%23+Org&homepage=&license=MIT&tags=org&org_id=acme"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        created.headers().get(header::LOCATION).unwrap(),
        "/plugins/acme/org-plugin?ok=created"
    );

    let detail = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/acme/org-plugin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert!(body_text(detail).await.contains("Org Plugin"));

    // 组织插件的版本也必须能下载：`find_asset` 早先只 JOIN users，
    // 组织（owner_id 为 NULL）发布的版本会 404——市场安装会直接踩到。
    let upload = upload_release(&ctx.app, &cookie, &csrf, "acme", "org-plugin", "0.1.0").await;
    assert_eq!(upload.status(), StatusCode::SEE_OTHER);
    let download = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/plugins/acme/org-plugin/releases/0.1.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_token_can_create_and_list_plugins() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "frank", "frank@example.com").await;

    let created = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/tokens")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}&name=CI&read=1&write=1")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let html = body_text(created).await;
    let token = extract_api_token(&html);

    let body = serde_json::json!({
        "slug": "api-plugin",
        "name": "API Plugin",
        "description": "created via api",
        "readme": "# API",
        "tags": "api"
    });
    let response = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/plugins")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let list = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    assert!(body_text(list).await.contains("api-plugin"));

    let no_token = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_token.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_can_be_created_and_oauth_reports_unconfigured() {
    let ctx = setup().await;
    let (cookie, csrf) = register(&ctx.app, "grace", "grace@example.com").await;

    let create = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/webhooks")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "_csrf={csrf}&url=http%3A%2F%2F127.0.0.1%3A9%2Fhook&plugin_created=1&release_published=1"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::SEE_OTHER);

    let page = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/settings/webhooks")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_text(page).await;
    assert!(html.contains("127.0.0.1:9/hook"));
    assert!(html.contains("plugin.created"));

    let oauth = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/github")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oauth.status(), StatusCode::BAD_REQUEST);
    assert!(body_text(oauth).await.contains("GitHub OAuth 未配置"));
}

fn extract_api_token(html: &str) -> String {
    let start = html.find("ada_pat_").expect("页面应显示新 token");
    let rest = &html[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

#[tokio::test]
async fn oauth_start_redirects_when_configured() {
    let dir = tempfile::tempdir().expect("临时目录");
    let pool = db::test_pool().await;
    let storage = Storage::new(dir.path().join("assets")).expect("资产目录");
    let mut config = Config::for_tests(dir.path().join("assets"));
    config.github_client_id = Some("client-id".to_string());
    config.github_client_secret = Some("client-secret".to_string());
    let state = AppState::new(pool, storage, config);
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/github")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.starts_with("https://github.com/login/oauth/authorize"));
    assert!(location.contains("client_id=client-id"));
    assert!(resp.headers().get(header::SET_COOKIE).is_some());
}
