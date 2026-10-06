//! LMR（Ling Memory Runtime）核心规则：记忆打分、粒度衰减、证据置信度。
//!
//! 设计原则（架构文档级，勿破坏）：
//! 1. 检索优先结构化，向量只做兜底；
//! 2. 遗忘只降低粒度（decay_level），不删除任何证据；
//! 3. 记忆/特质/关系统一为语义图（SPO + scope），星图只是它的视觉投影。
//!
//! 本模块只放**纯函数**（可独立测试），持久化在 [`crate::store::Store`]。

/// 归一化记忆评分的各分量（全部 0..=1）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreComponents {
    /// 语义相似度（检索器给出；无向量时由关键词/实体匹配近似）
    pub semantic: f32,
    /// 事件重要度
    pub importance: f32,
    /// 时新性（越新越高）
    pub recency: f32,
    /// 实体重合度
    pub entity_match: f32,
    /// 关系强度（与说话者的关系值）
    pub relationship: f32,
    /// 事实置信度
    pub confidence: f32,
}

/// 评分权重（默认档；按查询意图可换 preset）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreWeights {
    pub semantic: f32,
    pub importance: f32,
    pub recency: f32,
    pub entity_match: f32,
    pub relationship: f32,
    pub confidence: f32,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self { semantic: 0.30, importance: 0.20, recency: 0.15,
               entity_match: 0.15, relationship: 0.10, confidence: 0.10 }
    }
}

impl ScoreWeights {
    /// 「还记得第一次…吗」：时间与重要度优先，时新性让位
    pub fn temporal() -> Self {
        Self { semantic: 0.30, importance: 0.30, recency: 0.05,
               entity_match: 0.20, relationship: 0.05, confidence: 0.10 }
    }
    /// 「你为什么喜欢…」：语义与特质（实体）优先
    pub fn semantic() -> Self {
        Self { semantic: 0.40, importance: 0.20, recency: 0.05,
               entity_match: 0.20, relationship: 0.05, confidence: 0.10 }
    }
    /// 「我们刚才聊了什么」：时新性压倒一切
    pub fn recency() -> Self {
        Self { semantic: 0.10, importance: 0.10, recency: 0.60,
               entity_match: 0.10, relationship: 0.05, confidence: 0.05 }
    }

    fn sum(&self) -> f32 {
        self.semantic + self.importance + self.recency
            + self.entity_match + self.relationship + self.confidence
    }
}

/// 归一化加权评分（权重和不强制为 1，内部归一化）
pub fn score(c: &ScoreComponents, w: &ScoreWeights) -> f32 {
    let total = w.sum().max(1e-6);
    let v = w.semantic * c.semantic
        + w.importance * c.importance
        + w.recency * c.recency
        + w.entity_match * c.entity_match
        + w.relationship * c.relationship
        + w.confidence * c.confidence;
    (v / total).clamp(0.0, 1.0)
}

/// 粒度衰减：返回 decay_level（0=raw, 1=summary, 2=micro_summary）。
///
/// - `age_secs`：距场景发生的秒数；
/// - `importance`：所属事件的重要度（0..=1）——越重要衰减越慢；
/// - `half_life_secs`：半衰期。重要度 1.0 的记忆半衰期加倍体验（speed=1.5），
///   重要度 0 的记忆按原速衰减。
///
/// 只返回层级，不删除任何数据（raw 永久保留在库中）。
pub fn decay_level(age_secs: i64, importance: f32, half_life_secs: i64) -> u8 {
    if age_secs <= 0 || half_life_secs <= 0 {
        return 0;
    }
    let imp = importance.clamp(0.0, 1.0);
    // 重要度越高衰减越慢：有效半衰期 = half_life / (0.5 + importance)
    let speed = 0.5 + imp;
    let threshold = half_life_secs as f32 / speed;
    let age = age_secs as f32;
    if age >= threshold * 4.0 {
        2
    } else if age >= threshold {
        1
    } else {
        0
    }
}

/// 证据置信度重算：基准 0.5，按证据增量线性累计后夹在 [0.05, 1.0]。
///
/// `deltas`：每条证据的增量（正=确认，负=削弱）。
pub fn confidence_from_evidence(deltas: &[f32]) -> f32 {
    let sum: f32 = deltas.iter().sum();
    (0.5 + sum).clamp(0.05, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comps(semantic: f32, recency: f32) -> ScoreComponents {
        ScoreComponents {
            semantic, importance: 0.5, recency,
            entity_match: 0.5, relationship: 0.5, confidence: 0.5,
        }
    }

    #[test]
    fn score_stays_in_unit_range_and_respects_weights() {
        let s = score(&comps(1.0, 0.0), &ScoreWeights::default());
        assert!(s > 0.5 && s <= 1.0);
        // 相同分量、不同权重 → 分数不同（权重确实参与计算）
        let s2 = score(&comps(1.0, 0.0), &ScoreWeights::recency());
        assert!((s - s2).abs() > 0.05);
    }

    #[test]
    fn score_extremes_clamp() {
        let w = ScoreWeights::default();
        let all_one = ScoreComponents { semantic: 1.0, importance: 1.0, recency: 1.0,
                                        entity_match: 1.0, relationship: 1.0, confidence: 1.0 };
        assert!((score(&all_one, &w) - 1.0).abs() < 1e-5);
        let all_zero = ScoreComponents { semantic: 0.0, importance: 0.0, recency: 0.0,
                                         entity_match: 0.0, relationship: 0.0, confidence: 0.0 };
        assert_eq!(score(&all_zero, &w), 0.0);
    }

    #[test]
    fn decay_is_monotonic_and_importance_slows_it() {
        let age = 10 * half_life_secs();
        // 同龄时，重要度越高衰减层级越低
        assert!(decay_level(age, 0.9, half_life_secs()) <= decay_level(age, 0.1, half_life_secs()));
        // 时间越久层级越高
        assert!(decay_level(age, 0.5, half_life_secs()) >= decay_level(age / 4, 0.5, half_life_secs()));
    }

    #[test]
    fn decay_caps_at_two_and_never_negative() {
        assert_eq!(decay_level(10_000_000_000, 0.0, half_life_secs()), 2);
        assert_eq!(decay_level(0, 0.5, half_life_secs()), 0);
    }

    fn half_life_secs() -> i64 {
        30 * 24 * 3600 // 30 天（与测试自洽的任意常数）
    }

    #[test]
    fn confidence_recompute_clamps() {
        assert!((confidence_from_evidence(&[0.4, 0.2]) - 1.0).abs() < 1e-5);   // 0.5+0.6 → 顶格
        assert!((confidence_from_evidence(&[-0.3, -0.2]) - 0.05).abs() < 1e-5); // 底格
        assert!((confidence_from_evidence(&[0.4, -0.1]) - 0.8).abs() < 1e-5);  // 0.5+0.3
    }
}
