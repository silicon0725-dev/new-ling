//! 视觉参数换算：节点 / 边的颜色、半径、厚度（全部纯函数，克制白光基调）。
//!
//! 所有颜色输出为**线性 RGBA**（shader 侧目标是 sRGB 纹理，由硬件完成编码），
//! 且已按「预乘」约定给出：`rgb` 已经乘上对应的视觉贡献度，配合加法混合使用。

use crate::snapshot::{EdgeKind, NodeKind};

/// 一组节点配色（核心 + 光晕）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodePalette {
    /// 核心圆盘颜色（线性 RGBA，预乘）
    pub core: [f32; 4],
    /// 光晕颜色（线性 RGBA，预乘；alpha 本就不高，保持克制）
    pub halo: [f32; 4],
}

/// 深空背景色（线性 RGBA，清屏用）
pub const BACKGROUND: [f32; 4] = [0.012, 0.018, 0.035, 1.0];

/// 节点类别的基础色相（线性空间，趋近白但带一点冷暖倾向）
fn kind_tint(kind: NodeKind) -> [f32; 3] {
    match kind {
        // 角色：纯白微暖，最亮
        NodeKind::Character => [1.00, 0.98, 0.94],
        // 特质：暖白偏金
        NodeKind::Trait => [1.00, 0.90, 0.74],
        // 弧光：冷白偏蓝
        NodeKind::MemoryArc => [0.80, 0.89, 1.00],
        // 事件：青白
        NodeKind::MemoryEvent => [0.70, 0.85, 0.96],
    }
}

/// 节点配色换算：权重越高核心越实、光晕越亮（光晕上限克制在 0.45）。
///
/// `weight` 会被夹取到 0..1；`kind.base_weight()` 与 `weight` 相乘作为综合强度。
pub fn node_palette(kind: NodeKind, weight: f32) -> NodePalette {
    let weight = weight.clamp(0.0, 1.0);
    let intensity = (kind.base_weight() * weight).clamp(0.05, 1.0);
    let tint = kind_tint(kind);
    // 核心不透明度：0.72（沉寂）→ 1.0（活跃）——加法混合下核心应清晰可辨
    let core_alpha = 0.72 + 0.28 * intensity;
    // 光晕不透明度：0.10 → 0.45，指数上凸让高权重略突出
    let halo_alpha = 0.10 + 0.35 * intensity.powf(1.5);
    NodePalette {
        core: [
            tint[0] * core_alpha,
            tint[1] * core_alpha,
            tint[2] * core_alpha,
            core_alpha,
        ],
        halo: [
            tint[0] * halo_alpha,
            tint[1] * halo_alpha,
            tint[2] * halo_alpha,
            halo_alpha,
        ],
    }
}

/// 边类别的基础色相（线性空间）
fn edge_tint(kind: EdgeKind) -> [f32; 3] {
    match kind {
        // 归属：淡蓝白（结构感）
        EdgeKind::Ownership => [0.55, 0.68, 0.92],
        // 因果：暖琥珀白（时间感）
        EdgeKind::Causal => [0.94, 0.83, 0.62],
    }
}

/// 连线颜色换算：强度越高越亮，最高不过 0.70（连线仍弱于节点核心）。
pub fn edge_color(kind: EdgeKind, strength: f32) -> [f32; 4] {
    let strength = strength.clamp(0.0, 1.0);
    let alpha = 0.14 + 0.56 * strength;
    let tint = edge_tint(kind);
    [tint[0] * alpha, tint[1] * alpha, tint[2] * alpha, alpha]
}

/// 视口短边与基准分辨率（1080p）之比，用于整体缩放视觉尺寸。
/// 夹取在 0.5..2.0，避免极端窗口下节点过大 / 过小。
pub fn ui_scale(min_viewport_px: f32) -> f32 {
    if !min_viewport_px.is_finite() || min_viewport_px <= 0.0 {
        return 1.0;
    }
    (min_viewport_px / 1080.0).clamp(0.5, 2.0)
}

/// 节点半径（像素，指光晕外沿）：`sqrt` 综合强度映射到 7..32 像素。
pub fn node_radius_px(kind: NodeKind, weight: f32, min_viewport_px: f32) -> f32 {
    let weight = weight.clamp(0.0, 1.0);
    let intensity = (kind.base_weight() * weight).clamp(0.0, 1.0);
    (7.0 + 25.0 * intensity.sqrt()) * ui_scale(min_viewport_px)
}

/// 连线半厚度（像素）：约 1..4，因果边略细（它们靠流光而非粗细表达）。
pub fn edge_thickness_px(kind: EdgeKind, strength: f32, min_viewport_px: f32) -> f32 {
    let strength = strength.clamp(0.0, 1.0);
    let base = match kind {
        EdgeKind::Ownership => 1.5,
        EdgeKind::Causal => 1.2,
    };
    (base * (0.8 + 2.4 * strength)) * ui_scale(min_viewport_px)
}

/// 背景微粒子半径（像素）：1.2..2.8，越小越克制。
pub fn particle_radius_px(frand: f32, min_viewport_px: f32) -> f32 {
    let frand = if frand.is_finite() {
        frand.clamp(0.0, 1.0)
    } else {
        0.5
    };
    (1.2 + 1.6 * frand) * ui_scale(min_viewport_px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有分量为有限值且 rgb/alpha ∈ 0..1
    fn assert_sane_rgba(c: [f32; 4], what: &str) {
        for v in c {
            assert!(v.is_finite(), "{what} 出现非有限值：{c:?}");
            assert!((0.0..=1.0).contains(&v), "{what} 越界：{c:?}");
        }
    }

    #[test]
    fn palettes_are_in_range_and_premultiplied() {
        for kind in [
            NodeKind::Character,
            NodeKind::Trait,
            NodeKind::MemoryArc,
            NodeKind::MemoryEvent,
        ] {
            for &w in &[-0.5, 0.0, 0.3, 1.0, 1.7] {
                let palette = node_palette(kind, w);
                assert_sane_rgba(palette.core, "core");
                assert_sane_rgba(palette.halo, "halo");
                // 预乘约定：rgb ≤ alpha（tint ≤ 1）
                for slot in 0..3 {
                    assert!(palette.core[slot] <= palette.core[3] + 1e-6);
                    assert!(palette.halo[slot] <= palette.halo[3] + 1e-6);
                }
            }
        }
    }

    #[test]
    fn halo_stays_restrained() {
        // 光晕 alpha 上限 0.45：任何类别 / 权重组合都不越界
        for kind in [
            NodeKind::Character,
            NodeKind::Trait,
            NodeKind::MemoryArc,
            NodeKind::MemoryEvent,
        ] {
            assert!(node_palette(kind, 1.0).halo[3] <= 0.45 + 1e-6);
        }
    }

    #[test]
    fn brightness_monotonic_in_weight_and_kind() {
        // 同类别：权重↑ → 光晕更亮、半径更大
        for kind in [
            NodeKind::Character,
            NodeKind::Trait,
            NodeKind::MemoryArc,
            NodeKind::MemoryEvent,
        ] {
            assert!(node_palette(kind, 0.9).halo[3] > node_palette(kind, 0.2).halo[3]);
            assert!(node_radius_px(kind, 0.9, 1080.0) > node_radius_px(kind, 0.2, 1080.0));
        }
        // 同权重：类别基础权重↓ → 半径↓
        assert!(
            node_radius_px(NodeKind::Character, 0.8, 1080.0)
                > node_radius_px(NodeKind::MemoryArc, 0.8, 1080.0)
        );
        assert!(
            node_radius_px(NodeKind::MemoryArc, 0.8, 1080.0)
                > node_radius_px(NodeKind::MemoryEvent, 0.8, 1080.0)
        );
    }

    #[test]
    fn edge_colors_are_dimmer_than_node_cores() {
        let edge = edge_color(EdgeKind::Causal, 1.0);
        let core = node_palette(NodeKind::MemoryEvent, 0.2).core;
        // 连线（即便最强）不能比最弱节点核心更亮
        assert!(edge[3] < core[3]);
        assert_sane_rgba(edge, "edge");
    }

    #[test]
    fn ui_scale_clamps_extremes() {
        assert_eq!(ui_scale(1080.0), 1.0);
        assert_eq!(ui_scale(240.0), 0.5); // 下限
        assert_eq!(ui_scale(5000.0), 2.0); // 上限
        assert_eq!(ui_scale(0.0), 1.0); // 非法输入兜底
        assert_eq!(ui_scale(f32::NAN), 1.0);
    }

    #[test]
    fn radii_and_thickness_ranges() {
        // 1080p 下节点半径落在 7..32 像素
        assert!(node_radius_px(NodeKind::Character, 1.0, 1080.0) <= 32.0 + 1e-4);
        assert!(node_radius_px(NodeKind::MemoryEvent, 0.0, 1080.0) >= 7.0 - 1e-4);
        // 厚度正、有限，因果 < 归属（同强度）
        for kind in [EdgeKind::Ownership, EdgeKind::Causal] {
            let t = edge_thickness_px(kind, 1.0, 1080.0);
            assert!(t.is_finite() && t > 0.0 && t < 5.0);
        }
        assert!(
            edge_thickness_px(EdgeKind::Causal, 0.8, 1080.0)
                < edge_thickness_px(EdgeKind::Ownership, 0.8, 1080.0)
        );
        assert!((particle_radius_px(0.5, 1080.0) - 2.0).abs() < 1e-4);
    }
}
