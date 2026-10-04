//! # ling-core
//!
//! 「灵」角色扮演引擎的核心库（纯 Rust，不依赖任何 UI/GPU 框架）。
//!
//! 模块一览：
//! - [`character`]：角色 / 身份 / 特质（活跃度衰减、休眠与唤醒、对话涌现写入）
//! - [`memory`]：三层记忆引擎（scene → event → arc 逐层聚合与检索）
//! - [`world`]：领地与世界最小状态（1:72 时间流速，四季昼夜留数据结构与 TODO）
//! - [`ai`]：OpenAI 兼容客户端（SSE 流式 + 结构化 JSON，带 SSRF 防护）
//! - [`store`]：rusqlite(bundled) 持久化（编号迁移 + 全参数绑定）
//! - [`starmap`]：星图力导向布局模拟（确定性初始化、可收敛）

pub mod ai;
pub mod character;
pub mod memory;
pub mod starmap;
pub mod store;
pub mod world;
