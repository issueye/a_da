import { Button, Chip, Icon, Modal, Notice, Select } from "ssr-h:components";

function pluginPath(props: any, suffix: string) {
  return "/ext/" + props.plugin.id + "/providers" + suffix;
}

function providerHref(props: any, id: string, tab: string) {
  return pluginPath(props, "?provider=" + encodeURIComponent(id) + "&tab=" + tab);
}

function providerAction(id: string, suffix: string) {
  return "/providers/" + encodeURIComponent(id) + suffix;
}

export function Page(props: any) {
  const admin = props && props.data ? props.data.providerAdmin : null;
  if (!admin || !admin.providers || !admin.providers.length) {
    return (
      <div class="pa-page">
        <Notice
          text="没有读到供应商数据：请确认网关已连接，并给插件声明 providers.read 能力。"
          tone="warn"
        />
      </div>
    );
  }

  const providers = admin.providers;
  const selected =
    providers.find((provider: any) => provider.id === admin.selectedId) || providers[0];
  const tab = admin.activeTab === "models" ? "models" : "connection";
  const connectionAction = providerAction(selected.id, "");
  const modelAction = providerAction(selected.id, "/models");
  // 当前生效的模型归谁：链接跳到它的模型页签，而不是当前选中的供应商
  const activeProvider =
    providers.find((provider: any) => provider.active) || null;

  return (
    <div class="pa-page">
      <header class="pa-head">
        <div class="pa-head__left">
          <h1 class="pa-title">供应商</h1>
          {admin.currentModel ? (
            activeProvider ? (
              <a
                class="pa-current"
                href={providerHref(props, activeProvider.id, "models")}
                title="当前生效的模型，点击查看它的模型列表"
              >
                <span class="pa-current__label">当前模型</span>
                <strong class="pa-current__value">{admin.currentModel}</strong>
              </a>
            ) : (
              <span class="pa-current pa-current--static">
                <span class="pa-current__label">当前模型</span>
                <strong class="pa-current__value">{admin.currentModel}</strong>
              </span>
            )
          ) : null}
        </div>
        <form method="post" action="/providers/new">
          <Button label="新建供应商" variant="primary" size="sm" icon="i-plus" kind="submit" />
        </form>
      </header>

      <div class="pa-body">
        <div class="pa-layout">
          <aside class="pa-list">
            <div class="pa-list__head">已配置的供应商</div>
            <div class="pa-list__body">
              {providers.map((provider: any) => (
                <div class={"pa-provider" + (provider.id === selected.id ? " is-selected" : "")}>
                  <a class="pa-provider__link" href={providerHref(props, provider.id, tab)}>
                    <span class="pa-provider__name">{provider.name}</span>
                    <span class="pa-provider__meta">{provider.modelCount} 个模型 · {provider.protocolLabel}</span>
                  </a>
                  {provider.active ? <Chip text="当前" tone="success" /> : null}
                  {admin.canDeleteProviders ? (
                    <form method="post" action={providerAction(provider.id, "/delete")} data-confirm={"删除供应商「" + provider.name + "」？"}>
                      <button class="pa-provider__delete" type="submit" aria-label="删除供应商" title="删除供应商">
                        <Icon name="i-trash" size={13} />
                      </button>
                    </form>
                  ) : null}
                </div>
              ))}
            </div>
          </aside>

          <section class="pa-detail">
            <nav class="pa-tabs" aria-label="供应商详情">
              <a class={"pa-tab" + (tab === "connection" ? " is-active" : "")} href={providerHref(props, selected.id, "connection")}>供应商信息</a>
              <a class={"pa-tab" + (tab === "models" ? " is-active" : "")} href={providerHref(props, selected.id, "models")}>模型列表<span class="pa-tab__count">{selected.modelCount}</span></a>
            </nav>

            {tab === "connection" ? (
              <div class="pa-pane">
                <form method="post" action={connectionAction} class="pa-form">
                  <div class="pa-grid">
                    <label class="pa-field"><span>名称</span><input class="ada-input" name="name" value={selected.name} /></label>
                    <label class="pa-field"><span>协议</span>
                      <Select name="protocol" value={selected.protocol} title="协议" options={admin.protocols.map((item: any) => ({ value: item.value, label: item.label }))} />
                    </label>
                  </div>
                  <label class="pa-field"><span>API 端点 (Base URL)</span><input class="ada-input" name="baseUrl" value={selected.baseUrl} /></label>
                  <div class="pa-grid">
                    <label class="pa-field"><span>API Key {selected.apiKeyHint ? "（已配置 " + selected.apiKeyHint + "）" : ""}</span><input class="ada-input" type="password" name="apiKey" placeholder="留空表示不修改" /></label>
                    <label class="pa-field"><span>代理 URL（可选）</span><input class="ada-input" name="proxyUrl" value={selected.proxyUrl} /></label>
                  </div>
                  <label class="pa-field"><span>自定义请求头（可选）</span><textarea class="ada-input pa-textarea" name="headers" rows={4} placeholder="格式 k: v; k2: v2">{selected.headers}</textarea></label>
                  <div class="pa-checks">
                    <label class="pa-check"><input type="checkbox" name="clearApiKey" value="1" />清空已保存的密钥</label>
                    <label class="pa-check"><input type="checkbox" name="makeActive" value="1" checked={selected.active} />设为当前供应商</label>
                  </div>
                  <div class="pa-actions">
                    <Button label="保存连接" variant="primary" size="sm" kind="submit" />
                    <button class="btn btn-secondary btn-sm" type="button" hx-post={providerAction(selected.id, "/test")} hx-target="#pa-test-result" hx-swap="innerHTML" hx-include="closest form">测试连接</button>
                    <span id="pa-test-result"></span>
                  </div>
                </form>
              </div>
            ) : (
              <div class="pa-pane pa-pane--models">
                <div class="pa-models-head">
                  <div class="pa-actions">
                    <button class="btn btn-secondary btn-sm" type="button" data-modal-open="remote-models">获取远程模型</button>
                    <button class="btn btn-primary btn-sm" type="button" data-modal-open="model-new">新增模型</button>
                  </div>
                </div>


                <div class="pa-table-scroll mt-3">
                  <table class="table ada-model-table">
                    <colgroup>
                      <col class="ada-col-id" /><col class="ada-col-name" /><col class="ada-col-context" /><col class="ada-col-output" /><col class="ada-col-image" /><col class="ada-col-status" /><col class="ada-col-actions" />
                    </colgroup>
                    <thead><tr><th>模型 ID</th><th>显示名</th><th>上下文</th><th>最大输出</th><th>图片</th><th>状态</th><th>操作</th></tr></thead>
                    <tbody>
                      {selected.models.map((model: any) => (
                        <tr>
                          <td class="font-mono text-[11px] text-ink" title={model.id}>{model.id}</td>
                          <td class="text-xs" title={model.name}>{model.name}</td>
                          <td class="font-mono text-[11px]">{model.contextWindow}</td>
                          <td class="font-mono text-[11px]">{model.maxOutputTokens}</td>
                          <td class="text-[10px]">{model.supportsImages ? <span class="badge badge-neutral">支持</span> : <span class="text-faint">—</span>}</td>
                          <td>{model.current ? <span class="badge badge-success">当前</span> : <span class="text-[10px] text-faint">—</span>}</td>
                          <td class="ada-model-table__actions">
                            <details class="ada-row-menu">
                              <summary class="ada-row-menu__trigger" aria-label="模型操作" title="模型操作">
                                <Icon name="i-more" size={15} />
                              </summary>
                              <div class="ada-row-menu__panel" role="menu">
                                <button class="ada-row-menu__item" type="button" role="menuitem" data-modal-open={"model-" + model.index}>编辑模型</button>
                                {!model.current ? (
                                  <form method="post" action={providerAction(selected.id, "/activate")}>
                                    <input type="hidden" name="model" value={model.id} />
                                    <button class="ada-row-menu__item" type="submit" role="menuitem">设为当前</button>
                                  </form>
                                ) : null}
                                <form method="post" action={providerAction(selected.id, "/models/" + model.index + "/delete")} data-confirm={"移除模型 " + model.id + "？"}>
                                  <button class="ada-row-menu__item ada-row-menu__item--danger" type="submit" role="menuitem" disabled={selected.modelCount <= 1}>移除模型</button>
                                </form>
                              </div>
                            </details>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>

                <div id="pa-remote-loader" hidden data-modal-src="remote-models" hx-post={providerAction(selected.id, "/models/fetch")} hx-trigger="ada-modal-load" hx-target="#pa-remote-models-body" hx-swap="innerHTML"></div>

                <Modal id="remote-models" title="获取远程模型" wide>
                  <div class="pa-remote-toolbar">
                    <span class="pa-modal-hint">远端模型列表</span>
                    <button class="btn btn-secondary btn-sm" type="button" data-modal-close>关闭</button>
                  </div>
                  <div id="pa-remote-models-body">
                    <p class="pa-modal-hint">正在读取远端模型列表…</p>
                  </div>
                </Modal>
                {selected.models.map((model: any) => (
                  <Modal id={"model-" + model.index} title={"编辑模型 · " + model.name}>
                    <form method="post" action={providerAction(selected.id, "/models/" + model.index)} class="pa-form">
                      <div class="pa-grid">
                        <label class="pa-field"><span>模型 ID</span><input class="ada-input" name="id" value={model.id} /></label>
                        <label class="pa-field"><span>显示名</span><input class="ada-input" name="name" value={model.name} /></label>
                        <label class="pa-field"><span>上下文窗口</span><input class="ada-input" name="contextWindow" value={model.contextWindow} /></label>
                        <label class="pa-field"><span>最大输出</span><input class="ada-input" name="maxOutputTokens" value={model.maxOutputTokens} /></label>
                      </div>
                      <label class="pa-check"><input type="checkbox" name="supportsImages" value="1" checked={model.supportsImages} />支持图片输入</label>
                      <div class="pa-modal-actions">
                        <button class="btn btn-secondary btn-sm" type="button" data-modal-close>取消</button>
                        <Button label="保存模型" variant="primary" size="sm" kind="submit" />
                      </div>
                    </form>
                  </Modal>
                ))}

                <Modal id="model-new" title="新增模型">
                  <form method="post" action={modelAction} class="pa-form">
                    <div class="pa-grid">
                      <label class="pa-field"><span>模型 ID</span><input class="ada-input" name="id" placeholder="例如 deepseek/deepseek-v4" /></label>
                      <label class="pa-field"><span>显示名</span><input class="ada-input" name="name" placeholder="可选" /></label>
                      <label class="pa-field"><span>上下文窗口</span><input class="ada-input" name="contextWindow" value="1000000" /></label>
                      <label class="pa-field"><span>最大输出</span><input class="ada-input" name="maxOutputTokens" value="65536" /></label>
                    </div>
                    <label class="pa-check"><input type="checkbox" name="supportsImages" value="1" checked />支持图片输入</label>
                    <div class="pa-modal-actions">
                      <button class="btn btn-secondary btn-sm" type="button" data-modal-close>取消</button>
                      <Button label="添加模型" variant="primary" size="sm" kind="submit" />
                    </div>
                  </form>
                </Modal>
              </div>
            )}
          </section>
        </div>
      </div>

    </div>
  );
}









