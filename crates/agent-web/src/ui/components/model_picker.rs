//! 模型选择器（参考 DeepSeek 的模型菜单）。
//!
//! 一枚工具行药丸 + 一张向上弹出的面板：
//!
//! - **模型**：按供应商分组的模型清单，点一条就切到"该供应商 + 该模型"；
//! - **推理等级**：低 / 中 / 高 / 自动（`max`），写进网关的 `config.setEffort`。
//!
//! 为什么是 `<details>` + 表单而不是 JS 下拉：无 JS 时菜单照样能开、能提交，
//! JS 只做"点外面关闭 / 同层只开一个分组"这类增强（见 `static/chat.js`）。
//! 药丸的标签单独用一个 `#composer-model` 容器承载，live 轮询只替换它的内容，
//! 不会把展开中的菜单关掉。

use serde_json::Value;

use super::Render;
use crate::ui::icons::icon;
use crate::ui::theme::escape;

/// 一个可选模型。
#[derive(Debug, Clone, Default)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
}

/// 一个供应商分组。
#[derive(Debug, Clone, Default)]
pub struct ModelGroup {
    pub provider_id: String,
    pub provider_name: String,
    pub models: Vec<ModelChoice>,
}

/// 模型选择器。
#[derive(Debug, Clone, Default)]
pub struct ModelPicker {
    /// 切换后跳回的会话（空串 = 回首页）。
    pub thread_id: String,
    /// 当前模型 id（与 `config.model` 同源）。
    pub model: String,
    /// 当前推理等级：`low | medium | high | max`。
    pub effort: String,
    /// 当前激活的供应商（用来判断"当前模型"该打哪一条）。
    pub active_provider_id: String,
    pub groups: Vec<ModelGroup>,
}

impl ModelPicker {
    /// 从 `/api/providers` 的响应构造（`providers[]` 里每一项带 `models[]`）。
    pub fn from_providers(
        values: &Value,
        active_provider_id: &str,
        model: &str,
        effort: &str,
        thread_id: &str,
    ) -> Self {
        let groups = values
            .get("providers")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|provider| {
                let provider_id = str_at(provider, "id").to_string();
                let models: Vec<ModelChoice> = provider
                    .get("models")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .map(|model| {
                        let id = str_at(model, "id").to_string();
                        let name = {
                            let name = str_at(model, "name");
                            if name.is_empty() {
                                id.clone()
                            } else {
                                name.to_string()
                            }
                        };
                        ModelChoice { id, name }
                    })
                    .filter(|model| !model.id.trim().is_empty())
                    .collect();
                if models.is_empty() {
                    return None;
                }
                Some(ModelGroup {
                    provider_id,
                    provider_name: {
                        let name = str_at(provider, "name");
                        if name.is_empty() {
                            str_at(provider, "id").to_string()
                        } else {
                            name.to_string()
                        }
                    },
                    models,
                })
            })
            .collect();
        Self {
            thread_id: thread_id.to_string(),
            model: model.to_string(),
            effort: effort.to_string(),
            active_provider_id: active_provider_id.to_string(),
            groups,
        }
    }

    fn current_choice(&self) -> Option<(&ModelGroup, &ModelChoice)> {
        self.groups
            .iter()
            .find(|group| group.provider_id == self.active_provider_id)
            .and_then(|group| {
                group
                    .models
                    .iter()
                    .find(|model| model.id == self.model)
                    .map(|choice| (group, choice))
            })
    }

    fn current_label(&self) -> String {
        self.current_choice()
            .map(|(_, choice)| choice.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.model.clone())
    }

    fn model_list(&self) -> String {
        let mut out = String::new();
        for group in &self.groups {
            out.push_str(&format!(
                r#"<p class="ada-model-picker__provider">{}</p>"#,
                escape(&group.provider_name)
            ));
            for choice in &group.models {
                let current =
                    group.provider_id == self.active_provider_id && choice.id == self.model;
                // 名称和 id 相同时只显示一行：同一串字重复两遍只是噪音。
                let id_html = if choice.name == choice.id {
                    String::new()
                } else {
                    format!(
                        r#"<span class="ada-model-picker__item-id">{}</span>"#,
                        escape(&choice.id)
                    )
                };
                out.push_str(&format!(
                    r#"<form method="post" action="/model/select" class="ada-model-picker__form" data-partial>
  <input type="hidden" name="provider" value="{provider}">
  <input type="hidden" name="model" value="{model}">
  <input type="hidden" name="thread" value="{thread}">
  <button type="submit" class="ada-model-picker__item{current}"{aria}>
    <span class="ada-model-picker__item-copy">
      <span class="ada-model-picker__item-name">{name}</span>
      {id}
    </span>
    <span class="ada-model-picker__check">{check}</span>
  </button>
</form>"#,
                    provider = escape(&group.provider_id),
                    model = escape(&choice.id),
                    thread = escape(&self.thread_id),
                    current = if current { " is-current" } else { "" },
                    aria = if current {
                        r#" aria-current="true""#
                    } else {
                        ""
                    },
                    name = escape(&choice.name),
                    id = id_html,
                    check = if current {
                        icon("i-check", 12)
                    } else {
                        String::new()
                    },
                ));
            }
        }
        out
    }

    fn effort_list(&self) -> String {
        const LEVELS: [(&str, &str, &str); 4] = [
            ("max", "自动 (Auto)", "不指定，由模型按自身默认策略决定"),
            ("low", "低 (Low)", "快速直接响应，节省 Token"),
            ("medium", "中 (Medium)", "平衡速度与推理质量"),
            ("high", "高 (High)", "充分推理，权衡多种备选方案"),
        ];
        let current = normalize_effort(&self.effort);
        let mut out = String::new();
        for (value, label, desc) in LEVELS {
            let selected = value == current;
            out.push_str(&format!(
                r#"<form method="post" action="/model/effort" class="ada-model-picker__form" data-partial>
  <input type="hidden" name="thread" value="{thread}">
  <button type="submit" name="effort" value="{value}" class="ada-model-picker__item{current}"{aria}>
    <span class="ada-model-picker__item-copy">
      <span class="ada-model-picker__item-name">{label}</span>
      <span class="ada-model-picker__item-id">{desc}</span>
    </span>
    <span class="ada-model-picker__check">{check}</span>
  </button>
</form>"#,
                thread = escape(&self.thread_id),
                value = value,
                current = if selected { " is-current" } else { "" },
                aria = if selected {
                    r#" aria-current="true""#
                } else {
                    ""
                },
                label = escape(label),
                desc = escape(desc),
                check = if selected {
                    icon("i-check", 12)
                } else {
                    String::new()
                },
            ));
        }
        out
    }
}

impl Render for ModelPicker {
    fn render(&self) -> String {
        format!(
            r#"<details class="ada-model-picker" data-model-picker>
  <summary class="ada-tool ada-tool--model ada-model-picker__summary" title="当前模型：{model} · 推理等级：{effort}（点击切换）" aria-label="切换模型与推理等级">
    <span class="ada-model-picker__chip" id="composer-model">{chip}</span>
  </summary>
  <div class="ada-model-picker__panel">
    <details class="ada-model-picker__group">
      <summary class="ada-model-picker__row">
        <span class="ada-model-picker__row-label">模型</span>
        <span class="ada-model-picker__row-value">{model}</span>
        {chevron}
      </summary>
      <div class="ada-model-picker__list">{models}</div>
    </details>
    <details class="ada-model-picker__group">
      <summary class="ada-model-picker__row">
        <span class="ada-model-picker__row-label">推理等级</span>
        <span class="ada-model-picker__row-value">{effort}</span>
        {chevron}
      </summary>
      <div class="ada-model-picker__list">{efforts}</div>
    </details>
    <a class="ada-model-picker__manage" href="/ext/provider-admin/providers">
      {manage_icon}<span>管理供应商与模型</span>
    </a>
  </div>
</details>"#,
            chip = model_chip_inner(&self.model, &self.effort),
            model = escape(&self.current_label()),
            effort = effort_label(&self.effort),
            chevron = icon("i-chevron", 12),
            models = self.model_list(),
            efforts = self.effort_list(),
            manage_icon = icon("i-external", 12),
        )
    }
}

/// 药丸内部内容（与 live 轮询 OOB 共用，避免两份标签逻辑）。
///
/// 外层由 [`ModelPicker`] 套上 `#composer-model`；轮询只替换它，菜单的开合状态不受影响。
pub fn model_chip_inner(model: &str, effort: &str) -> String {
    // 空等级（例如"未连接"的离线态）不挂徽标：`Auto` 在那种语境下是假信息。
    let effort = if effort.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<span class="ada-model-picker__effort">{}</span>"#,
            effort_label(effort)
        )
    };
    format!(
        r#"{sparkles}<span class="ada-tool__label">{model}</span>{effort}{caret}"#,
        sparkles = icon("i-sparkles", 14),
        model = escape(model),
        caret = icon("i-chevron-down", 12),
    )
}

/// 推理等级 → 药丸/菜单里的短标签。
pub fn effort_label(value: &str) -> &'static str {
    match normalize_effort(value) {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        _ => "Auto",
    }
}

/// 未知/空值一律按 `max`（自动）处理——后端新增档位时不会渲染成空白。
pub fn normalize_effort(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        _ => "max",
    }
}

fn str_at<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn providers() -> Value {
        json!({
            "activeProviderId": "senseaudio",
            "providers": [
                {
                    "id": "senseaudio",
                    "name": "SenseAudio",
                    "models": [
                        { "id": "deepseek/deepseek-v4.1-flash", "name": "DeepSeek V4.1 Flash" },
                        { "id": "deepseek/deepseek-v4.1", "name": "DeepSeek V4.1" }
                    ]
                },
                {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": [{ "id": "gpt-5.6", "name": "GPT-5.6" }]
                },
                { "id": "empty", "name": "空供应商", "models": [] }
            ]
        })
    }

    #[test]
    fn renders_grouped_models_and_marks_current() {
        let picker = ModelPicker::from_providers(
            &providers(),
            "senseaudio",
            "deepseek/deepseek-v4.1-flash",
            "high",
            "t1",
        );
        let html = picker.render();
        assert!(html.starts_with("<details"), "{html}");
        assert!(html.contains("SenseAudio"), "{html}");
        assert!(html.contains("OpenAI"), "{html}");
        assert!(!html.contains("空供应商"), "空分组不该出现: {html}");
        assert!(html.contains("DeepSeek V4.1 Flash"), "{html}");
        // 当前模型 / 当前等级各只有一个勾选标记。
        assert_eq!(html.matches("is-current").count(), 2, "{html}");
        assert!(
            html.contains(r#"name="provider" value="senseaudio""#),
            "{html}"
        );
        assert!(html.contains(r#"action="/model/select""#), "{html}");
        assert!(html.contains(r#"action="/model/effort""#), "{html}");
        assert!(html.contains("High"), "{html}");
        // 药丸里的模型名是完整 id（live 轮询替换的是同一段内容）。
        assert!(
            html.contains(r#"<span class="ada-tool__label">deepseek/deepseek-v4.1-flash</span>"#),
            "{html}"
        );
    }

    #[test]
    fn marks_current_model_only_for_the_active_provider() {
        // 同一个模型 id 在别的供应商里不算"当前"，否则切换供应商时会出现两个勾。
        let values = json!({
            "providers": [
                { "id": "a", "name": "A", "models": [{ "id": "shared", "name": "Shared" }] },
                { "id": "b", "name": "B", "models": [{ "id": "shared", "name": "Shared" }] }
            ]
        });
        let picker = ModelPicker::from_providers(&values, "b", "shared", "max", "t1");
        assert_eq!(picker.render().matches("is-current").count(), 2); // 一条模型 + 一条等级
    }

    #[test]
    fn unknown_effort_falls_back_to_auto() {
        assert_eq!(effort_label(""), "Auto");
        assert_eq!(effort_label("weird"), "Auto");
        assert_eq!(effort_label("HIGH"), "High");
        assert_eq!(normalize_effort(" Low "), "low");
        assert_eq!(normalize_effort("max"), "max");
    }

    #[test]
    fn empty_effort_omits_the_badge() {
        // 离线态的"未连接"药丸不该挂一个 `Auto` 徽标（那是假信息）。
        let offline = model_chip_inner("未连接", "");
        assert!(!offline.contains("ada-model-picker__effort"), "{offline}");
        assert!(offline.contains("未连接"), "{offline}");
        let online = model_chip_inner("m", "high");
        assert!(online.contains(">High</span>"), "{online}");
    }

    #[test]
    fn empty_provider_list_still_renders_a_usable_menu() {
        let picker = ModelPicker::from_providers(&Value::Null, "", "m", "max", "t1");
        let html = picker.render();
        assert!(html.contains("管理供应商与模型"), "{html}");
        assert!(html.contains("推理等级"), "{html}");
    }
}

