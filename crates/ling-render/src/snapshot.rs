//! 星图快照：渲染器与 ling-core 领域数据之间的一次性「装载数据包」。
//!
//! 快照由节点（[`NodeVisual`]）与边（[`EdgeVisual`]）组成：
//! - 节点分四类：角色 / 特质 / 弧光记忆 / 事件记忆（[`NodeKind`]）；
//! - 边分两类：归属（角色→特质、弧光→事件）与因果（事件时间链）（[`EdgeKind`]）。
//!
//! 节点 `id` 只是**快照内的局部句柄**（装载时会被重排为 `0..n`），
//! 与数据库主键无必然关系，由上层负责生成时保证唯一。

use ling_core::character::Character;
use ling_core::memory::{MemoryArc, MemoryEvent};

/// 节点类别：决定基础尺寸、配色与光晕强度
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// 角色节点（星图中心，最亮最大）
    Character,
    /// 特质节点（角色性格 / 能力标签，暖白偏金）
    Trait,
    /// 弧光记忆节点（长线剧情，冷白偏蓝）
    MemoryArc,
    /// 事件记忆节点（具体情节，青白，最小）
    MemoryEvent,
}

impl NodeKind {
    /// 类别基础权重（0..1）：越大节点越大、光晕越强
    pub fn base_weight(self) -> f32 {
        match self {
            NodeKind::Character => 1.00,
            NodeKind::MemoryArc => 0.72,
            NodeKind::Trait => 0.60,
            NodeKind::MemoryEvent => 0.45,
        }
    }
}

/// 边类别：归属连线（结构）与因果连线（时序）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// 归属：角色→特质、弧光→事件
    Ownership,
    /// 因果：事件→下一事件（按时间链）
    Causal,
}

/// 一个待渲染的节点
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeVisual {
    /// 快照内局部句柄（装载后重排为 0..n）
    pub id: i64,
    /// 节点类别
    pub kind: NodeKind,
    /// 个体权重 0..1（特质→活跃度；记忆→层次启发式权重）
    pub weight: f32,
}

/// 一条待渲染的边
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeVisual {
    pub from: i64,
    pub to: i64,
    /// 边类别
    pub kind: EdgeKind,
    /// 强度 0..1（影响亮度 / 厚度）
    pub strength: f32,
}

/// 节点 / 边快照（一次性装载，之后每帧渲染它）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StarSnapshot {
    pub nodes: Vec<NodeVisual>,
    pub edges: Vec<EdgeVisual>,
}

impl StarSnapshot {
    /// 从 ling-core 领域数据（角色 + 事件记忆 + 弧光记忆）构建星图快照。
    ///
    /// 节点：角色 1 个、每个特质 1 个、每条弧光 / 事件各 1 个；
    /// 边：角色→特质（归属，强度=特质活跃度）、弧光→其包含的事件（归属）、
    /// 事件按 `(start_time, id)` 排序后两两相连（因果）。
    ///
    /// 权重启发式（可随后续迭代调整）：
    /// - 特质：`activity`（0..1，越活跃越亮）；
    /// - 弧光：`0.5 + 0.5 × min(1, 事件数 / 6)`；
    /// - 事件：`0.4 + 0.3 × min(1, 关键词数 / 6)`。
    pub fn from_character_memory(
        character: &Character,
        events: &[MemoryEvent],
        arcs: &[MemoryArc],
    ) -> Self {
        let mut nodes = Vec::with_capacity(1 + character.traits.len() + events.len() + arcs.len());
        let mut edges = Vec::new();

        // 节点句柄按构建顺序分配（0 = 角色，其后特质、弧光、事件）
        let character_id = 0_i64;
        nodes.push(NodeVisual {
            id: character_id,
            kind: NodeKind::Character,
            weight: 1.0,
        });

        // 特质节点 + 角色→特质归属边
        for trait_item in &character.traits {
            let id = nodes.len() as i64;
            let weight = trait_item.activity.clamp(0.0, 1.0);
            nodes.push(NodeVisual {
                id,
                kind: NodeKind::Trait,
                weight,
            });
            edges.push(EdgeVisual {
                from: character_id,
                to: id,
                kind: EdgeKind::Ownership,
                strength: weight.max(0.25),
            });
        }

        // 弧光节点（先建弧光，便于事件挂靠）
        for arc in arcs {
            let id = nodes.len() as i64;
            let weight = 0.5 + 0.5 * (arc.event_ids.len() as f32 / 6.0).min(1.0);
            nodes.push(NodeVisual {
                id,
                kind: NodeKind::MemoryArc,
                weight,
            });
            // 角色→弧光归属边（成长线属于角色）：把记忆群锚定在角色附近，
            // 否则两群之间只剩斥力、布局会把它们推到画面两端
            edges.push(EdgeVisual {
                from: character_id,
                to: id,
                kind: EdgeKind::Ownership,
                strength: 0.45,
            });
            // 弧光→事件归属边（事件节点稍后创建，先记录弧光句柄）
            let arc_handle = id;
            for event_id in &arc.event_ids {
                if let Some(pos) = events.iter().position(|e| e.id == *event_id) {
                    let event_handle = (nodes.len() + pos) as i64;
                    edges.push(EdgeVisual {
                        from: arc_handle,
                        to: event_handle,
                        kind: EdgeKind::Ownership,
                        strength: 0.6,
                    });
                }
            }
        }

        // 事件节点（句柄 = 当前节点数 + 在 events 中的下标，与上面对齐）
        for event in events {
            let weight = 0.4 + 0.3 * (event.keywords.len() as f32 / 6.0).min(1.0);
            nodes.push(NodeVisual {
                id: nodes.len() as i64,
                kind: NodeKind::MemoryEvent,
                weight,
            });
        }

        // 事件因果链：按 (start_time, id) 稳定排序后顺序相连
        let event_base = nodes.len() - events.len();
        let mut order: Vec<usize> = (0..events.len()).collect();
        order.sort_by(|&a, &b| {
            (events[a].start_time, events[a].id).cmp(&(events[b].start_time, events[b].id))
        });
        for pair in order.windows(2) {
            edges.push(EdgeVisual {
                from: (event_base + pair[0]) as i64,
                to: (event_base + pair[1]) as i64,
                kind: EdgeKind::Causal,
                strength: 0.5,
            });
        }

        Self { nodes, edges }
    }

    /// 规整快照：权重 / 强度夹取到 0..1，去重复句柄，丢弃悬空边与自环，
    /// 并把节点句柄**重排为 0..n**（渲染器按此顺序对齐力导向模拟的星体下标）。
    pub fn sanitized(mut self) -> Self {
        // 1) 夹取权重并按原句柄去重（保留首个）
        let mut seen: Vec<i64> = Vec::with_capacity(self.nodes.len());
        self.nodes.retain(|node| {
            let keep = !seen.contains(&node.id);
            if keep {
                seen.push(node.id);
            }
            keep
        });
        for node in &mut self.nodes {
            node.weight = node.weight.clamp(0.0, 1.0);
        }

        // 2) 句柄重排：旧句柄 → 新句柄（0..n）
        let old_to_new: std::collections::HashMap<i64, i64> = seen
            .iter()
            .enumerate()
            .map(|(new_idx, old)| (*old, new_idx as i64))
            .collect();
        for (new_idx, node) in self.nodes.iter_mut().enumerate() {
            node.id = new_idx as i64;
        }

        // 3) 边重映射：悬空 / 自环 / 重复丢弃，强度夹取
        let mut kept: Vec<EdgeVisual> = Vec::with_capacity(self.edges.len());
        for mut edge in self.edges {
            let (Some(&from), Some(&to)) = (old_to_new.get(&edge.from), old_to_new.get(&edge.to))
            else {
                continue;
            };
            if from == to {
                continue;
            }
            edge.from = from;
            edge.to = to;
            edge.strength = edge.strength.clamp(0.0, 1.0);
            if !kept
                .iter()
                .any(|e| (e.from, e.to, e.kind) == (edge.from, edge.to, edge.kind))
            {
                kept.push(edge);
            }
        }
        self.edges = kept;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ling_core::character::{Character, Identity};

    /// 构造带两个特质的测试角色
    fn sample_character() -> Character {
        let mut c = Character::new(7, Identity::new("阿澈", "观星者", "北境", "游侠"), 1_000);
        c.emerge_trait("沉静", "寡言但观察敏锐", 1_000);
        c.emerge_trait("执拗", "认定的事不回头", 2_000);
        c
    }

    /// 构造三条事件记忆（时间乱序给足排序空间）与一条弧光
    fn sample_memory() -> (Vec<MemoryEvent>, Vec<MemoryArc>) {
        let events = vec![
            MemoryEvent {
                id: 11,
                summary: "初遇".into(),
                keywords: vec!["北境".into(), "雪".into()],
                scene_ids: vec![1],
                start_time: 3_000,
                end_time: 3_100,
            },
            MemoryEvent {
                id: 12,
                summary: "同行的第一夜".into(),
                keywords: vec!["篝火".into()],
                scene_ids: vec![2],
                start_time: 5_000,
                end_time: 5_050,
            },
            MemoryEvent {
                id: 13,
                summary: "分岔口".into(),
                keywords: vec![
                    "抉择".into(),
                    "a".into(),
                    "b".into(),
                    "c".into(),
                    "d".into(),
                    "e".into(),
                    "f".into(),
                    "g".into(),
                ],
                scene_ids: vec![3],
                start_time: 1_000,
                end_time: 1_200,
            },
        ];
        let arcs = vec![MemoryArc {
            id: 21,
            title: "北行".into(),
            summary: "同行弧线".into(),
            keywords: vec!["北境".into()],
            event_ids: vec![11, 12],
            start_time: 3_000,
            end_time: 5_050,
        }];
        (events, arcs)
    }

    #[test]
    fn builder_produces_expected_topology() {
        let character = sample_character();
        let (events, arcs) = sample_memory();
        let snapshot = StarSnapshot::from_character_memory(&character, &events, &arcs);

        // 1 角色 + 2 特质 + 1 弧光 + 3 事件
        assert_eq!(snapshot.nodes.len(), 7);
        assert_eq!(snapshot.nodes[0].kind, NodeKind::Character);
        assert_eq!(snapshot.nodes[1].kind, NodeKind::Trait);
        assert_eq!(snapshot.nodes[3].kind, NodeKind::MemoryArc);
        assert_eq!(snapshot.nodes[4].kind, NodeKind::MemoryEvent);

        // 边数：2 条角色→特质 + 1 条角色→弧光 + 2 条弧光→事件 + 2 条因果链
        let ownership = snapshot
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Ownership)
            .count();
        let causal = snapshot
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Causal)
            .count();
        assert_eq!((ownership, causal), (5, 2));

        // 特质权重 = 活跃度（emerge 后为 0.8）
        assert!((snapshot.nodes[1].weight - 0.8).abs() < 1e-6);

        // 事件因果链必须按时间升序：13(1_000) → 11(3_000) → 12(5_000)
        let causal_edges: Vec<&EdgeVisual> = snapshot
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Causal)
            .collect();
        let starts: Vec<i64> = causal_edges
            .iter()
            .map(|e| events[(e.from - 4) as usize].start_time)
            .collect();
        assert_eq!(starts, vec![1_000, 3_000]);

        // 关键词多的事件权重更高（13 有 8 个关键词封顶）
        assert!(snapshot.nodes[6].weight > snapshot.nodes[4].weight);
        // 弧光权重 = 0.5 + 0.5 × min(1, 2/6)
        assert!((snapshot.nodes[3].weight - (0.5 + 0.5 * 2.0 / 6.0)).abs() < 1e-6);
    }

    #[test]
    fn sanitize_reindexes_and_drops_invalid() {
        let snapshot = StarSnapshot {
            nodes: vec![
                NodeVisual {
                    id: 100,
                    kind: NodeKind::Character,
                    weight: 1.0,
                },
                NodeVisual {
                    id: 100,
                    kind: NodeKind::Trait,
                    weight: 0.5,
                }, // 重复句柄 → 丢弃
                NodeVisual {
                    id: 300,
                    kind: NodeKind::Trait,
                    weight: 2.0,
                }, // 超界权重 → 夹取
            ],
            edges: vec![
                EdgeVisual {
                    from: 100,
                    to: 300,
                    kind: EdgeKind::Ownership,
                    strength: 9.0,
                },
                EdgeVisual {
                    from: 100,
                    to: 999,
                    kind: EdgeKind::Ownership,
                    strength: 0.5,
                }, // 悬空 → 丢弃
                EdgeVisual {
                    from: 300,
                    to: 300,
                    kind: EdgeKind::Causal,
                    strength: 0.5,
                }, // 自环 → 丢弃
                EdgeVisual {
                    from: 100,
                    to: 300,
                    kind: EdgeKind::Ownership,
                    strength: 0.2,
                }, // 重复 → 丢弃
                EdgeVisual {
                    from: 300,
                    to: 100,
                    kind: EdgeKind::Causal,
                    strength: -1.0,
                }, // 反向因果保留
            ],
        };
        let clean = snapshot.sanitized();
        assert_eq!(clean.nodes.len(), 2);
        assert_eq!(clean.nodes[0].id, 0);
        assert_eq!(clean.nodes[1].id, 1);
        assert_eq!(clean.nodes[1].weight, 1.0); // 2.0 夹取
                                                // 只剩归属边与反向因果边
        assert_eq!(clean.edges.len(), 2);
        assert!(clean
            .edges
            .iter()
            .all(|e| e.strength >= 0.0 && e.strength <= 1.0));
        assert!(clean.edges.iter().all(|e| e.from >= 0
            && (e.from as usize) < clean.nodes.len()
            && e.to >= 0
            && (e.to as usize) < clean.nodes.len()));
    }

    #[test]
    fn sanitize_empty_snapshot_stays_empty() {
        let clean = StarSnapshot::default().sanitized();
        assert!(clean.nodes.is_empty() && clean.edges.is_empty());
    }
}
