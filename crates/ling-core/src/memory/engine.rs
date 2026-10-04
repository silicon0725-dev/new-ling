//! 三层记忆聚合与检索引擎。

use std::cmp::Ordering;

use super::retriever::Retriever;
use super::{KeywordRetriever, MemoryArc, MemoryEvent, Scene};

/// 默认聚合阈值：相邻记忆相似度达到该值即并入同一上层记忆
pub const DEFAULT_MERGE_THRESHOLD: f32 = 0.15;

/// 三层记忆引擎。
///
/// 聚合规则（自下而上）：
/// 1. scene → event：按时间排序后顺序扫描，与当前簇累计文本相似度达标则并入，否则另起一簇；
/// 2. event → arc：同样的顺序聚类，作用于「摘要 + 关键词」文本。
///
/// 检索规则：先按 [`Retriever`] 的相似度打分（当前为关键词实现），
/// 后续可切换向量检索而不改动引擎。
pub struct MemoryEngine<R: Retriever> {
    retriever: R,
    /// 聚合阈值（用于不带 threshold 参数的便捷方法）
    pub merge_threshold: f32,
    scenes: Vec<Scene>,
    events: Vec<MemoryEvent>,
    arcs: Vec<MemoryArc>,
    next_scene_id: i64,
    next_event_id: i64,
    next_arc_id: i64,
}

impl MemoryEngine<KeywordRetriever> {
    /// 使用关键词检索器与默认阈值的引擎
    pub fn new() -> Self {
        Self::with_retriever(KeywordRetriever, DEFAULT_MERGE_THRESHOLD)
    }
}

impl Default for MemoryEngine<KeywordRetriever> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Retriever> MemoryEngine<R> {
    /// 使用自定义检索器与阈值构建引擎
    pub fn with_retriever(retriever: R, merge_threshold: f32) -> Self {
        Self {
            retriever,
            merge_threshold,
            scenes: Vec::new(),
            events: Vec::new(),
            arcs: Vec::new(),
            next_scene_id: 1,
            next_event_id: 1,
            next_arc_id: 1,
        }
    }

    pub fn retriever(&self) -> &R {
        &self.retriever
    }

    /// 记录一条场景记忆
    pub fn record_scene(&mut self, content: &str, keywords: Vec<String>, timestamp: i64) -> Scene {
        let scene = Scene {
            id: self.next_scene_id,
            content: content.to_string(),
            keywords,
            timestamp,
        };
        self.next_scene_id += 1;
        self.scenes.push(scene.clone());
        scene
    }

    pub fn scenes(&self) -> &[Scene] {
        &self.scenes
    }

    pub fn events(&self) -> &[MemoryEvent] {
        &self.events
    }

    pub fn arcs(&self) -> &[MemoryArc] {
        &self.arcs
    }

    /// 纯计算：把当前全部场景聚合为事件（不改变引擎状态），id 从当前计数器起编
    pub fn aggregate_scenes(&self) -> Vec<MemoryEvent> {
        self.aggregate_scenes_with(self.merge_threshold)
    }

    /// 同 [`aggregate_scenes`]，但显式指定相似度阈值
    pub fn aggregate_scenes_with(&self, threshold: f32) -> Vec<MemoryEvent> {
        let mut ordered: Vec<&Scene> = self.scenes.iter().collect();
        ordered.sort_by_key(|s| s.timestamp);

        let mut events = Vec::new();
        let mut cluster: Vec<&Scene> = Vec::new();
        let mut cluster_text = String::new();
        let mut next_event_id = self.next_event_id;

        for scene in ordered {
            let attach = !cluster.is_empty()
                && self.retriever.similarity(&scene.content, &cluster_text) >= threshold;
            if !attach && !cluster.is_empty() {
                events.push(build_event(next_event_id, &cluster));
                next_event_id += 1;
                cluster.clear();
                cluster_text.clear();
            }
            cluster.push(scene);
            cluster_text.push_str(&scene.content);
        }
        if !cluster.is_empty() {
            events.push(build_event(next_event_id, &cluster));
        }
        events
    }

    /// 聚合并写入引擎状态（重建事件列表），返回事件数
    pub fn refresh_events(&mut self) -> usize {
        self.refresh_events_with(self.merge_threshold)
    }

    /// 同 [`refresh_events`]，但显式指定相似度阈值
    pub fn refresh_events_with(&mut self, threshold: f32) -> usize {
        let events = self.aggregate_scenes_with(threshold);
        let count = events.len();
        self.next_event_id += count as i64;
        self.events = events;
        count
    }

    /// 纯计算：把已写入的事件聚合为弧光（需先 `refresh_events`）
    pub fn aggregate_events(&self) -> Vec<MemoryArc> {
        self.aggregate_events_with(self.merge_threshold)
    }

    /// 同 [`aggregate_events`]，但显式指定相似度阈值
    pub fn aggregate_events_with(&self, threshold: f32) -> Vec<MemoryArc> {
        let mut ordered: Vec<&MemoryEvent> = self.events.iter().collect();
        ordered.sort_by_key(|e| e.start_time);

        let mut arcs = Vec::new();
        let mut cluster: Vec<&MemoryEvent> = Vec::new();
        let mut cluster_text = String::new();
        let mut next_arc_id = self.next_arc_id;

        for event in ordered {
            let text = event_text(event);
            let attach = !cluster.is_empty()
                && self.retriever.similarity(&text, &cluster_text) >= threshold;
            if !attach && !cluster.is_empty() {
                arcs.push(build_arc(next_arc_id, &cluster));
                next_arc_id += 1;
                cluster.clear();
                cluster_text.clear();
            }
            cluster.push(event);
            cluster_text.push_str(&text);
        }
        if !cluster.is_empty() {
            arcs.push(build_arc(next_arc_id, &cluster));
        }
        arcs
    }

    /// 聚合并写入引擎状态（重建弧光列表），返回弧光数
    pub fn refresh_arcs(&mut self) -> usize {
        self.refresh_arcs_with(self.merge_threshold)
    }

    /// 同 [`refresh_arcs`]，但显式指定相似度阈值
    pub fn refresh_arcs_with(&mut self, threshold: f32) -> usize {
        let arcs = self.aggregate_events_with(threshold);
        let count = arcs.len();
        self.next_arc_id += count as i64;
        self.arcs = arcs;
        count
    }

    /// 关键词/简单相似度检索场景，按得分降序，过滤零分，最多 limit 条
    pub fn search_scenes(&self, query: &str, limit: usize) -> Vec<(f32, &Scene)> {
        let mut scored: Vec<(f32, &Scene)> = self
            .scenes
            .iter()
            .map(|s| (self.retriever.similarity(query, &s.content), s))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
        scored
            .into_iter()
            .filter(|(score, _)| *score > 0.0)
            .take(limit)
            .collect()
    }

    /// 检索事件（打分文本 = 摘要 + 关键词）
    pub fn search_events(&self, query: &str, limit: usize) -> Vec<(f32, &MemoryEvent)> {
        let mut scored: Vec<(f32, &MemoryEvent)> = self
            .events
            .iter()
            .map(|e| (self.retriever.similarity(query, &event_text(e)), e))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
        scored
            .into_iter()
            .filter(|(score, _)| *score > 0.0)
            .take(limit)
            .collect()
    }
}

/// 事件的打分文本：摘要 + 关键词拼接
fn event_text(event: &MemoryEvent) -> String {
    format!("{} {}", event.summary, event.keywords.join(" "))
}

/// 由一簇场景构造事件
fn build_event(id: i64, cluster: &[&Scene]) -> MemoryEvent {
    // 关键词按首见顺序取并集
    let mut keywords: Vec<String> = Vec::new();
    for scene in cluster {
        for kw in &scene.keywords {
            if !keywords.iter().any(|k| k == kw) {
                keywords.push(kw.clone());
            }
        }
    }
    let joined = cluster
        .iter()
        .map(|s| s.content.as_str())
        .collect::<Vec<_>>()
        .join("；");
    let summary = truncate_chars(&joined, 120);
    MemoryEvent {
        id,
        summary,
        keywords,
        scene_ids: cluster.iter().map(|s| s.id).collect(),
        start_time: cluster.first().map(|s| s.timestamp).unwrap_or(0),
        end_time: cluster.last().map(|s| s.timestamp).unwrap_or(0),
    }
}

/// 由一簇事件构造弧光
fn build_arc(id: i64, cluster: &[&MemoryEvent]) -> MemoryArc {
    let mut keywords: Vec<String> = Vec::new();
    for event in cluster {
        for kw in &event.keywords {
            if !keywords.iter().any(|k| k == kw) {
                keywords.push(kw.clone());
            }
        }
    }
    let title = if keywords.is_empty() {
        "未命名弧光".to_string()
    } else {
        keywords.iter().take(3).cloned().collect::<Vec<_>>().join("·")
    };
    let joined = cluster
        .iter()
        .map(|e| e.summary.as_str())
        .collect::<Vec<_>>()
        .join("；");
    let summary = truncate_chars(&joined, 160);
    MemoryArc {
        id,
        title,
        summary,
        keywords,
        event_ids: cluster.iter().map(|e| e.id).collect(),
        start_time: cluster.first().map(|e| e.start_time).unwrap_or(0),
        end_time: cluster.last().map(|e| e.end_time).unwrap_or(0),
    }
}

/// 按字符数截断，超长补省略号
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max).collect();
        cut.push('…');
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::retriever::KeywordRetriever;

    fn seeded_engine() -> MemoryEngine<KeywordRetriever> {
        let mut engine = MemoryEngine::new();
        engine.record_scene("星图布局收敛", vec!["星图".into()], 1_000);
        engine.record_scene("星图布局模拟", vec!["星图".into()], 2_000);
        engine.record_scene("领地四季昼夜", vec!["领地".into()], 3_000);
        engine.record_scene("领地稳定度变化", vec!["领地".into()], 4_000);
        engine
    }

    #[test]
    fn scenes_aggregate_into_events() {
        let engine = seeded_engine();
        let events = engine.aggregate_scenes();
        assert_eq!(events.len(), 2, "两个主题应聚成两个事件");
        assert_eq!(events[0].scene_ids, vec![1, 2]);
        assert_eq!(events[1].scene_ids, vec![3, 4]);
        assert_eq!(events[0].start_time, 1_000);
        assert_eq!(events[0].end_time, 2_000);
        assert!(events[0].summary.contains("星图"));
        assert_eq!(events[0].keywords, vec!["星图".to_string()]);
    }

    #[test]
    fn events_aggregate_into_arc() {
        let mut engine = MemoryEngine::with_retriever(KeywordRetriever, 0.45);
        engine.record_scene("星图布局开始", vec!["星图".into()], 100);
        engine.record_scene("星图布局完成", vec!["星图".into()], 200);
        engine.record_scene("星图渲染开始", vec!["星图".into()], 300);
        engine.record_scene("星图渲染完成", vec!["星图".into()], 400);
        // 场景聚成两个事件（布局 / 渲染）
        assert_eq!(engine.refresh_events_with(0.45), 2);
        // 两个事件主题相近，聚成一条弧光
        assert_eq!(engine.refresh_arcs_with(0.5), 1);
        let arc = &engine.arcs()[0];
        assert_eq!(arc.event_ids, vec![1, 2]);
        assert!(arc.title.contains("星图"));
        assert_eq!(arc.start_time, 100);
        assert_eq!(arc.end_time, 400);
        // 一个事件包含多个场景
        assert_eq!(engine.events()[0].scene_ids, vec![1, 2]);
        assert_eq!(engine.events()[1].scene_ids, vec![3, 4]);
    }

    #[test]
    fn search_ranks_relevant_scenes_first() {
        let mut engine = seeded_engine();
        engine.record_scene("星图渲染提速", vec!["星图".into()], 5_000);
        let hits = engine.search_scenes("星图布局", 10);
        assert!(!hits.is_empty());
        // 最相关的是包含「布局」的场景
        assert!(hits[0].1.content.contains("布局"));
        // 得分降序
        assert!(hits[0].0 >= hits[1].0);
        // 零分被过滤：领地主题场景不出现
        assert!(hits.iter().all(|(_, s)| s.content.contains("星图")));
        // limit 生效
        assert_eq!(engine.search_scenes("星图布局", 1).len(), 1);
    }

    #[test]
    fn custom_retriever_can_replace_strategy() {
        /// 恒等于 1.0 的检索器：验证 trait 可替换（换成向量检索同理）
        struct AlwaysSimilar;
        impl Retriever for AlwaysSimilar {
            fn similarity(&self, _query: &str, _text: &str) -> f32 {
                1.0
            }
        }
        let mut engine = MemoryEngine::with_retriever(AlwaysSimilar, 0.9);
        engine.record_scene("甲", vec![], 1);
        engine.record_scene("乙", vec![], 2);
        engine.record_scene("丙", vec![], 3);
        assert_eq!(engine.refresh_events(), 1);
        assert_eq!(engine.events()[0].scene_ids, vec![1, 2, 3]);
    }

    #[test]
    fn empty_engine_returns_nothing() {
        let engine: MemoryEngine<KeywordRetriever> = MemoryEngine::new();
        assert!(engine.search_scenes("任意", 5).is_empty());
        assert!(engine.aggregate_scenes().is_empty());
        assert!(engine.aggregate_events().is_empty());
    }
}
