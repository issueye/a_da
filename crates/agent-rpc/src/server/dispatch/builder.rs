use super::*;

impl Dispatcher {
    pub fn new(
        store: Arc<RwLock<AgentStore>>,
        session_mgr: Arc<SessionManager>,
        checkpoint_mgr: Arc<CheckpointManager>,
        subagent_mgr: Arc<SubagentManager>,
        approval_mgr: Arc<ApprovalManager>,
        plugin_mgr: Arc<PluginManager>,
        skill_mgr: Arc<SkillManager>,
        broadcaster: Option<Arc<StateBroadcaster>>,
    ) -> Self {
        let store_bg = store.clone();
        let session_mgr_bg = session_mgr.clone();
        let broadcaster_bg = broadcaster.clone();
        tokio::spawn(async move {
            // 🔴 **整树会话同步很贵，不能每 1.5s 跑一次**。
            //
            // 实测（用户报"点批准要等 2–4 秒→四段日志实测 ②→③ 6437ms"）：
            // `sync_threads_from_disk` 会把 `sessions/` 下**每个**会话文件重新解析一遍
            // （实测环境有 2.8MB + 1.3MB + 454KB + … 的会话），是**秒级** CPU 活儿；
            // 它还要握着 store 写锁，而 WS 读取循环是逐条 await 的——
            // 于是批准/发消息这类请求只能在它后面排队，表现就是"点了没反应"。
            //
            // 先做**止损式**限流：间隔 1.5s → 10s，并把耗时打进日志（`[sync]`），
            // 让"它到底多贵"变成可见数字。真正的修法是 mtime 过滤（只解析改过的文件），
            // 见 docs/protocol/state-patch.md 的后续项——那时才能把间隔调回来。
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let started = std::time::Instant::now();
                let mut s = store_bg.write().await;
                let changed = s.sync_threads_from_disk(&session_mgr_bg);
                drop(s);
                let ms = started.elapsed().as_millis();
                if ms >= 200 {
                    tracing::warn!(
                        "[sync] 整树会话同步耗时 {}ms（changed={}）——会话文件越多越贵，待做 mtime 过滤",
                        ms,
                        changed
                    );
                }
                if changed {
                    if let Some(ref bc) = broadcaster_bg {
                        bc.mark_dirty();
                    }
                }
            }
        });

        Self {
            store,
            session_mgr,
            checkpoint_mgr,
            subagent_mgr,
            approval_mgr,
            plugin_mgr,
            skill_mgr,
            broadcaster,
            host_pid: std::process::id(),
            session_id: uuid::Uuid::new_v4().to_string(),
            abort_senders: Arc::new(Mutex::new(HashMap::new())),
            running_tasks: Arc::new(Mutex::new(HashMap::new())),
            engine: None,
            engine_factory: None,
            workspace_engines: Arc::new(Mutex::new(HashMap::new())),
            product_spec: None,
            pm_engine: None,
            pm_engine_factory: None,
            pm_workspace_engines: Arc::new(Mutex::new(HashMap::new())),
            plugin_skills: agent_node::skills::new_skill_store(Vec::new()),
            plugin_reassembler: None,
            hook_runner: None,
            plugin_contribution_views: None,
        }
    }

    /// P3a·SP-55：注入钩子运行时（管理面点位用）。没注入 = 这些点位不跑（如实）。
    pub fn with_hook_runner(mut self, runner: Arc<dyn agent_base::ports::HookRunner>) -> Self {
        self.hook_runner = Some(runner);
        self
    }

    /// P4：把"**实际装上了什么**"并进静态扫描结果（`plugin.list` / `plugin.load` 共用）。
    ///
    /// 实现是自由函数 [`merge_contribution_views`]（可单独测，不必造一整个 Dispatcher）。
    pub(super) fn merge_contribution_views(&self, items: &mut [agent_node::plugins::PluginItem]) {
        merge_contribution_views(self.plugin_contribution_views.as_ref(), items);
    }

    /// P2·SP-45：注入插件声明的技能（组合根在装配期收好的那份）。
    ///
    /// 保持 builder 风格的理由同 [`Self::with_engine`]：既有调用点与测试一行不改，
    /// 没注入时 `skill.list` 就只报目录与内置技能（**如实**，不是假装有插件技能）。
    pub fn with_plugin_skills(mut self, skills: agent_node::skills::SkillStore) -> Self {
        self.plugin_skills = skills;
        self
    }

    /// P2·SP-46：注入重装配入口（组合根实现）。没注入 = `plugin.setEnabled` 只写盘不重装配。
    pub fn with_plugin_reassembler(mut self, r: Arc<dyn PluginReassembler>) -> Self {
        self.plugin_reassembler = Some(r);
        self
    }

    /// P4：注入插件贡献的活体视图（组合根在装配期写、启停时原地替换）。
    ///
    /// 没注入 = `plugin.list` 的贡献字段如实为空（那是"这台宿主没执行过插件"的真相，
    /// 而不是"插件没有任何贡献"）。
    pub fn with_plugin_contribution_views(
        mut self,
        views: agent_node::plugins::PluginContributionStore,
    ) -> Self {
        self.plugin_contribution_views = Some(views);
        self
    }

    /// W3-T2：注入真引擎（`AgentRuntime`）。宿主装配层（`agent-host`）构造后传进来。
    ///
    /// 保持 builder 风格：既有调用点与测试**一行不改**，只有真正要切引擎的宿主才调它。
    pub fn with_engine(mut self, engine: Arc<agent_base::engine::AgentRuntime>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// 注入针对多工作区的动态编码引擎工厂
    pub fn with_engine_factory(mut self, factory: EngineFactory) -> Self {
        self.engine_factory = Some(factory);
        self
    }

    /// 注入 PM 助手真引擎。
    pub fn with_pm_engine(mut self, engine: Arc<agent_base::engine::AgentRuntime>) -> Self {
        self.pm_engine = Some(engine);
        self
    }

    /// 注入针对多工作区的动态 PM 引擎工厂
    pub fn with_pm_engine_factory(mut self, factory: EngineFactory) -> Self {
        self.pm_engine_factory = Some(factory);
        self
    }

    /// 同上，但接受 `Option`——宿主"有就注入、没有就保持 legacy"的形态。
    ///
    /// ⚠️ 这里**只取 `runtime`**：`approval_mgr` 必须由调用方在构造 `Dispatcher` 时就传对
    /// （见 [`EngineInjection`] 的说明），事后替换会留下"闸门挂在旧表上"的坑。
    pub fn pipe_engine(mut self, injection: Option<EngineInjection>) -> Self {
        if let Some(i) = injection {
            self.engine = Some(i.runtime);
            // W3-T6：声明一起带进来，握手才能如实回报能力位
            self.product_spec = Some(i.spec);
            self.engine_factory = i.factory;
        }
        self
    }

    /// 管道式注入 **PM 模式**引擎（`ada-coding` 的 PM 模式预设，不是第二个产品）。
    ///
    /// 注意**只取 runtime 与 factory，不碰 `product_spec`**：握手回报的产品身份
    /// 必须始终是主引擎（`ada-coding`）那份声明——PM 只是同一产品的一个模式，
    /// 若让预设的 `id`（`ada-coding-pm`）漏进 `productInfo`，界面就会把它当成产品 id 用。
    pub fn pipe_pm_engine(mut self, injection: Option<EngineInjection>) -> Self {
        if let Some(i) = injection {
            self.pm_engine = Some(i.runtime);
            self.pm_engine_factory = i.factory;
        }
        self
    }

    /// 只注入产品声明（W3-T6）：用于"没有引擎也要如实回报能力位"的场景与测试。
    pub fn with_product_spec(mut self, spec: Arc<agent_runtime::AgentSpec>) -> Self {
        self.product_spec = Some(spec);
        self
    }

    /// 是否已注入真引擎。
    ///
    /// W3-T4 之后**必须**为 `true`：legacy 主循环已删除，"没引擎"不再是可运行状态。
    pub fn has_engine(&self) -> bool {
        self.engine.is_some()
    }
}
