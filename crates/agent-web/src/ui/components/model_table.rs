//! 供应商模型表：固定列宽、省略号 + 行级操作菜单。

use crate::ui::theme::escape;

use super::Render;

/// 一个模型行（与 `ProviderModelRow` 同形，便于模板/组件共用）。
#[derive(Debug, Clone)]
pub struct ModelTableRow {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub context_window: String,
    pub max_output_tokens: String,
    pub supports_images: bool,
    pub current: bool,
}

/// 供应商模型列表组件。
pub struct ModelTable<'a> {
    provider_id: &'a str,
    provider_name: &'a str,
    rows: &'a [ModelTableRow],
}

impl<'a> ModelTable<'a> {
    pub fn new(provider_id: &'a str, provider_name: &'a str, rows: &'a [ModelTableRow]) -> Self {
        Self {
            provider_id,
            provider_name,
            rows,
        }
    }
}

impl Render for ModelTable<'_> {
    fn render(&self) -> String {
        let mut rows = String::new();
        for model in self.rows {
            let current = if model.current {
                r#"<span class="badge badge-success">当前</span>"#.to_string()
            } else {
                r#"<span class="text-[10px] text-faint">—</span>"#.to_string()
            };
            let images = if model.supports_images {
                r#"<span class="badge badge-neutral">支持</span>"#.to_string()
            } else {
                r#"<span class="text-faint">—</span>"#.to_string()
            };
            let activate = if model.current {
                String::new()
            } else {
                format!(
                    r#"<form method="post" action="/providers/{provider}/activate">
  <input type="hidden" name="model" value="{model_id}">
  <button class="ada-row-menu__item" type="submit" role="menuitem">设为当前</button>
</form>"#,
                    provider = escape(self.provider_id),
                    model_id = escape(&model.id),
                )
            };
            let delete_disabled = self.rows.len() <= 1;
            let disabled = if delete_disabled {
                r#" disabled title="每个供应商至少保留一个模型""#
            } else {
                ""
            };
            rows.push_str(&format!(
                r#"<tr>
  <td class="font-mono text-[11px] text-ink" title="{id_attr}">{id}</td>
  <td class="text-xs" title="{name_attr}">{name}</td>
  <td class="font-mono text-[11px]" title="{context_attr}">{context}</td>
  <td class="font-mono text-[11px]" title="{output_attr}">{output}</td>
  <td class="text-[10px]">{images}</td>
  <td>{current}</td>
  <td class="ada-model-table__actions">
    <details class="ada-row-menu">
      <summary class="ada-row-menu__trigger" aria-label="模型操作" title="模型操作">
        <svg width="15" height="15" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
          <circle cx="3" cy="8" r="1.5"/>
          <circle cx="8" cy="8" r="1.5"/>
          <circle cx="13" cy="8" r="1.5"/>
        </svg>
      </summary>
      <div class="ada-row-menu__panel" role="menu">
        <button class="ada-row-menu__item" type="button" role="menuitem" data-modal-open="model-{index}">编辑模型</button>
        {activate}
        <form method="post" action="/providers/{provider}/models/{index}/delete"
              data-confirm="从供应商「{provider_name}」移除模型 {model_id}？">
          <button class="ada-row-menu__item ada-row-menu__item--danger" type="submit" role="menuitem"{disabled}>移除模型</button>
        </form>
      </div>
    </details>
  </td>
</tr>"#,
                id = escape(&model.id),
                id_attr = escape(&model.id),
                name = escape(&model.name),
                name_attr = escape(&model.name),
                context = escape(&model.context_window),
                context_attr = escape(&model.context_window),
                output = escape(&model.max_output_tokens),
                output_attr = escape(&model.max_output_tokens),
                index = model.index,
                provider = escape(self.provider_id),
                provider_name = escape(self.provider_name),
                model_id = escape(&model.id),
                images = images,
                current = current,
                activate = activate,
                disabled = disabled,
            ));
        }

        format!(
            r#"<div class="ada-table-wrap mt-3">
  <table class="table ada-model-table">
    <colgroup>
      <col class="ada-col-id">
      <col class="ada-col-name">
      <col class="ada-col-context">
      <col class="ada-col-output">
      <col class="ada-col-image">
      <col class="ada-col-status">
      <col class="ada-col-actions">
    </colgroup>
    <thead>
      <tr>
        <th>模型 ID</th><th>显示名</th><th>上下文</th><th>最大输出</th><th>图片</th><th>状态</th><th>操作</th>
      </tr>
    </thead>
    <tbody>{rows}</tbody>
  </table>
</div>"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(index: usize, id: &str, current: bool) -> ModelTableRow {
        ModelTableRow {
            index,
            id: id.to_string(),
            name: format!("模型 {index}"),
            context_window: "1000000".to_string(),
            max_output_tokens: "65536".to_string(),
            supports_images: true,
            current,
        }
    }

    #[test]
    fn table_is_fixed_width_and_exposes_full_values_via_title() {
        let rows = vec![
            row(0, "deepseek/deepseek-v4.1-flash", false),
            row(1, "minimax/m3", true),
        ];
        let html = ModelTable::new("prov_1", "供应商", &rows).render();
        assert!(html.contains(r#"<table class="table ada-model-table">"#));
        assert!(html.contains(r#"<col class="ada-col-id">"#));
        assert!(html.contains(r#"title="deepseek/deepseek-v4.1-flash""#));
        assert!(html.contains("编辑模型"));
        assert!(html.contains("设为当前"));
        assert!(html.contains("移除模型"));
    }

    #[test]
    fn last_model_cannot_be_removed() {
        let rows = vec![row(0, "only-model", true)];
        let html = ModelTable::new("prov_1", "供应商", &rows).render();
        assert!(!html.contains("设为当前"));
        assert!(html.contains("disabled title=\"每个供应商至少保留一个模型\""));
    }
}
