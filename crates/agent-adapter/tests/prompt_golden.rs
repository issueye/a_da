//! 提示词**逐字节**回归（P2 · SP-42）。
//!
//! # 为什么这是不可放宽的一条
//!
//! SP-41 把提示词从"一串 `push_str`"改成"有序片段 + 纯拼接"。这类重构最容易出的事是
//! **排版悄悄变了**：多一个空行、少一个换行、技能段落的顺序换了——而提示词一字节的差别
//! 就是模型看到的东西不同，属于**行为变更**而不是重构。
//!
//! 所以夹具是**重构前**用当时的代码跑出来的（`tests/fixtures/prompt/*.txt`），
//! 而不是照着新实现写的——照着新实现写等于自己给自己判卷。
//!
//! 夹具覆盖三条路径：最朴素（无身份/无扩展/无技能）、声明身份 + 扩展工具、
//! 英文语言环境 + 两条技能（分别走 `when_to_use` 与 `description` 两个分支）。

use std::path::Path;

use agent_adapter::prompt::{
    build_prompt_plan, compose_system_prompt, compose_system_prompt_full, ProductIdentity,
    SkillPrompt,
};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("prompt")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读黄金夹具失败（{}）：{e}", path.display()))
}

fn extensions() -> Vec<(String, String)> {
    vec![
        ("web_search".to_string(), "联网搜索".to_string()),
        ("rss".to_string(), "订阅 RSS".to_string()),
    ]
}

fn skills() -> Vec<SkillPrompt> {
    vec![
        SkillPrompt::new("TDD", "测试驱动开发", "先写测试，再写实现。")
            .with_when_to_use("当用户要求写测试时"),
        SkillPrompt::new("Commit", "提交规范", "提交信息用祈使句。"),
    ]
}

/// 三条路径都必须与**重构前捕获**的输出逐字节一致。
#[test]
fn test_prompt_is_byte_identical_to_captured_golden() {
    let plain = compose_system_prompt("/tmp/ws", &[]);
    assert_eq!(
        plain,
        fixture("plain.txt"),
        "最朴素路径（无身份/无扩展/无技能）必须逐字节一致"
    );

    let identity = ProductIdentity::new(
        "a_da 编程助手",
        "你是一个严谨且专业的 AI 编程助手",
        "zh-CN",
    );
    let with_extensions = compose_system_prompt_full("/tmp/ws", &extensions(), Some(&identity), &[]);
    assert_eq!(
        with_extensions,
        fixture("identity_extensions.txt"),
        "身份 + 扩展段落必须逐字节一致"
    );

    let identity_en = ProductIdentity::new("a_da coder", "a careful coding agent", "en-US");
    let with_skills = compose_system_prompt_full("/tmp/ws", &[], Some(&identity_en), &skills());
    assert_eq!(
        with_skills,
        fixture("identity_skills_en.txt"),
        "英文语言环境 + 两条技能（覆盖 when_to_use 与 description 两个分支）必须逐字节一致"
    );
}

/// 片段必须真的解释得清"这些字节是从哪来的"（SP-43 的埋点就靠它）。
///
/// 断言的是**结构**而不是文案：四段固有片段都在、技能**逐条**成片段（而不是糊成一大块）、
/// 且各段字节数之和 == 渲染长度（没有凭空多出或少掉字节）。
#[test]
fn test_fragments_explain_every_byte() {
    let identity = ProductIdentity::new("a_da coder", "agent", "en-US");
    let plan = build_prompt_plan("/tmp/ws", &extensions(), Some(&identity), &skills());

    let ids: Vec<&str> = plan.fragments().iter().map(|f| f.id.as_str()).collect();
    for expected in ["persona", "workspace", "guidelines", "language", "extensions"] {
        assert!(ids.contains(&expected), "缺少固有片段 `{expected}`：{ids:?}");
    }
    assert!(ids.contains(&"skills_header"), "{ids:?}");
    assert!(ids.contains(&"skill:TDD") && ids.contains(&"skill:Commit"), "{ids:?}");

    let rendered = plan.render();
    assert_eq!(
        plan.total_bytes(),
        rendered.len(),
        "各片段字节数之和必须等于渲染长度（不许有拼接时凭空多出的字节）"
    );
    assert_eq!(
        rendered,
        compose_system_prompt_full("/tmp/ws", &extensions(), Some(&identity), &skills()),
        "plan.render() 与 compose_system_prompt_full 必须同源"
    );

    // 来源标注：这几段是编码助手的固有排版（插件片段将来会带 `plugin:<id>`）
    assert!(
        plan.byte_report()
            .iter()
            .all(|r| r.source == "builtin:core"),
        "固有片段的来源标签必须是 builtin:core"
    );
    // 技能正文的字节数要能单独看出来（"哪个技能占了多少"）
    let tdd = plan
        .byte_report()
        .into_iter()
        .find(|r| r.id == "skill:TDD")
        .expect("有 TDD 片段");
    assert!(tdd.bytes > 0);
}

/// SP-43：端口报的**逐段字节**必须与真正发出去的提示词自洽。
///
/// 这条与 conformance 的契约断言同一个性质，但走的是**端口的真实实现**
/// （`CodingPromptSource`，含技能解析器）——埋点报的数字必须是发出去的那一份算出来的。
#[test]
fn test_port_reports_segments_consistent_with_the_prompt_it_serves() {
    use agent_adapter::prompt::CodingPromptSource;
    use agent_base::ports::PromptSource;

    let source = CodingPromptSource::new("/tmp/ws")
        .with_identity(ProductIdentity::new("a_da coder", "agent", "en-US"));

    let prompt = source.system_prompt();
    let segments = source.prompt_segments();
    assert!(
        !segments.is_empty(),
        "真实实现必须逐段报告（否则埋点只剩一个总长度）"
    );
    assert_eq!(
        segments.iter().map(|s| s.bytes).sum::<usize>(),
        prompt.len(),
        "逐段字节之和必须等于提示词长度（不自洽的拆分比不拆更坏）"
    );
    let ids: Vec<&str> = segments.iter().map(|s| s.id.as_str()).collect();
    for expected in ["persona", "workspace", "guidelines", "language"] {
        assert!(ids.contains(&expected), "缺少 `{expected}`：{ids:?}");
    }
    assert!(
        segments.iter().all(|s| s.source == "builtin:core"),
        "固有片段的来源标签必须是 builtin:core"
    );
}

/// SP-44：插件片段排在固有段落**之后**、来源标 `plugin:<id>`，而且账目自洽。
///
/// "只在后面追加"这条是硬要求：插件不许改动固有段落（那是**只收窄不放宽**在提示词上的形态）。
#[test]
fn test_plugin_fragments_render_after_builtins_with_plugin_source() {
    use agent_adapter::prompt::CodingPromptSource;
    use agent_base::ports::PromptSource;

    let base = CodingPromptSource::new("/tmp/ws").system_prompt();
    let source = CodingPromptSource::new("/tmp/ws").with_plugin_fragments(vec![(
        "p1".to_string(),
        "house-style".to_string(),
        "本项目一律用 tabs。".to_string(),
    )]);
    let prompt = source.system_prompt();

    assert!(
        prompt.starts_with(&base),
        "固有段落必须原样保留（插件只能在后面追加）"
    );
    assert!(
        prompt.ends_with("本项目一律用 tabs。"),
        "插件片段必须落在末尾：{prompt}"
    );

    let segments = source.prompt_segments();
    let last = segments.last().expect("有片段");
    assert_eq!(last.source, "plugin:p1", "来源必须能看出是哪个插件加的");
    assert_eq!(last.id, "plugin:p1:house-style");
    assert_eq!(
        segments.iter().map(|s| s.bytes).sum::<usize>(),
        prompt.len(),
        "加进插件片段后账目仍要自洽"
    );

    // 空片段不追加（否则只会多出空行）
    let blank = CodingPromptSource::new("/tmp/ws").with_plugin_fragments(vec![(
        "p1".to_string(),
        "blank".to_string(),
        "   ".to_string(),
    )]);
    assert_eq!(blank.system_prompt(), base, "空片段必须被忽略");
}
