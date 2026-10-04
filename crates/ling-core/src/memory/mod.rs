//! 三层记忆：scene（场景）→ event（事件）→ arc（弧光）。
//!
//! 层级关系：一条 arc（长线剧情/成长弧）包含多个 event，一个 event 包含多个 scene。
//! scene 是对话片段的最小记忆单元；引擎负责逐层聚合与检索。
//! 检索策略抽象为 [`Retriever`] trait，当前提供关键词实现，后续可无缝替换为向量检索。

pub mod engine;
pub mod retriever;

pub use engine::MemoryEngine;
pub use retriever::{KeywordRetriever, Retriever};

/// 场景记忆：最小记忆单元，对应一段对话片段
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub id: i64,
    /// 场景原文（或速记）
    pub content: String,
    /// 关键词（由记忆提炼产出，是检索与聚合的辅助信号）
    pub keywords: Vec<String>,
    /// 发生时间（Unix 秒）
    pub timestamp: i64,
}

/// 事件记忆：若干相邻且相似的 scene 聚合出的情节
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEvent {
    pub id: i64,
    /// 事件摘要
    pub summary: String,
    /// 关键词并集
    pub keywords: Vec<String>,
    /// 包含的 scene id 列表（按时间序）
    pub scene_ids: Vec<i64>,
    /// 起始时间（Unix 秒）
    pub start_time: i64,
    /// 结束时间（Unix 秒）
    pub end_time: i64,
}

/// 弧光记忆：长线剧情 / 角色成长弧，由多个 event 聚合而成
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryArc {
    pub id: i64,
    /// 弧光标题（取自关键词）
    pub title: String,
    /// 弧光摘要
    pub summary: String,
    pub keywords: Vec<String>,
    /// 包含的 event id 列表（按时间序）
    pub event_ids: Vec<i64>,
    pub start_time: i64,
    pub end_time: i64,
}
