//! 前端插件系统：清单、注册表与生命周期。
//!
//! 目标（v0.6）：
//!
//! - **可扩展**：插件通过插槽把 UI 挂到界面的固定位置，不必改主应用代码；
//! - **可热插拔**：新增/修改/删除插件目录后重载即可生效，不重启、不重编译；
//! - **不拖垮主应用**：任何插件的加载/渲染异常都被隔离，界面退回默认实现；
//! - **SSR/CSR 一致**：插件内容在服务端渲染进 HTML，客户端只做增强。

#![allow(dead_code, unused_imports)]

pub mod manifest;
pub mod registry;

pub use manifest::{Manifest, NavEntry, PageEntry, Slot, SlotEntry};
pub use registry::{
    dashboard_widgets_off_thread, read_page_off_thread, render_slot_off_thread,
    render_slots_off_thread, NavLink, PluginRecord, PluginRegistry, PluginSnapshot, PluginStatus,
    PluginWidget, ReloadReport, SlotOutput,
};
