use askama::Template;

use crate::models::org::{Org, OrgChoice, OrgMember, Team, TeamMember};
use crate::models::token::ApiToken;
use crate::models::webhook::{Delivery, Webhook};
use crate::models::{PluginCard, PluginDetail, PluginFormData, Release};

use super::{Common, Flash};

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {
    pub common: Common,
    pub latest: Vec<PluginCard>,
    pub top: Vec<PluginCard>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub users: i64,
    pub plugins: i64,
    pub releases: i64,
}

#[derive(Template)]
#[template(path = "browse.html")]
pub struct BrowseTemplate {
    pub common: Common,
    pub plugins: Vec<PluginCard>,
    pub total: i64,
    pub page: i64,
    pub pages: i64,
    pub sort: String,
    pub q: String,
    pub q_url: String,
}

#[derive(Template)]
#[template(path = "plugin.html")]
pub struct PluginTemplate {
    pub common: Common,
    pub plugin: PluginDetail,
    pub releases: Vec<Release>,
    pub starred: bool,
    pub stars: i64,
    pub can_edit: bool,
    pub readme_html: String,
}

#[derive(Template)]
#[template(path = "plugin_form.html")]
pub struct PluginFormTemplate {
    pub common: Common,
    pub form: PluginFormData,
    pub orgs: Vec<OrgChoice>,
    pub action: String,
    pub heading: String,
    pub submit: String,
    pub error: Option<String>,
    /// 非空时在表单顶部显示「从最新版本插件包同步」按钮（编辑页才会给）。
    pub sync_action: String,
}

#[derive(Template)]
#[template(path = "release_form.html")]
pub struct ReleaseFormTemplate {
    pub common: Common,
    pub owner: String,
    pub slug: String,
    pub name: String,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "auth.html")]
pub struct AuthTemplate {
    pub common: Common,
    pub heading: String,
    pub action: String,
    pub submit: String,
    pub next: String,
    pub error: Option<String>,
    pub github_enabled: bool,
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct ProfileTemplate {
    pub common: Common,
    pub profile: ProfileView,
    pub plugins: Vec<PluginCard>,
}

#[derive(Debug, Clone)]
pub struct ProfileView {
    pub username: String,
    pub joined: String,
    pub plugin_count: i64,
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate {
    pub common: Common,
    pub status: u16,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct StarTarget {
    pub owner: String,
    pub slug: String,
}

#[derive(Template)]
#[template(path = "partials/star_button.html")]
pub struct StarButtonTemplate {
    pub common: Common,
    pub plugin: StarTarget,
    pub starred: bool,
    pub stars: i64,
}

#[derive(Template)]
#[template(path = "partials/search_results.html")]
pub struct SearchResultsTemplate {
    pub plugins: Vec<PluginCard>,
    pub q: String,
}

#[derive(Template)]
#[template(path = "orgs.html")]
pub struct OrgsTemplate {
    pub common: Common,
    pub orgs: Vec<Org>,
}

#[derive(Template)]
#[template(path = "org_new.html")]
pub struct OrgNewTemplate {
    pub common: Common,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "org.html")]
pub struct OrgTemplate {
    pub common: Common,
    pub org: Org,
    pub plugins: Vec<PluginCard>,
    pub members: Vec<OrgMember>,
    pub teams: Vec<TeamView>,
    pub can_manage: bool,
    pub my_role: String,
}

#[derive(Debug, Clone)]
pub struct TeamView {
    pub team: Team,
    pub members: Vec<TeamMember>,
}

#[derive(Template)]
#[template(path = "tokens.html")]
pub struct TokenPageTemplate {
    pub common: Common,
    pub tokens: Vec<ApiToken>,
    pub new_token: Option<String>,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "webhooks.html")]
pub struct WebhookPageTemplate {
    pub common: Common,
    pub webhooks: Vec<Webhook>,
    pub deliveries: Vec<Delivery>,
    pub new_webhook: Option<Webhook>,
    pub error: Option<String>,
}

pub fn flash(ok: Option<&str>, err: Option<&str>) -> Option<Flash> {
    Flash::from_query(ok, err)
}
