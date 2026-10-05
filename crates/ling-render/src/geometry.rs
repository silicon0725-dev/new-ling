//! 坐标变换：世界坐标（力导向模拟输出）↔ 像素坐标 ↔ 裁剪空间（NDC）。
//!
//! 视口适配策略：**等比缩放 + 居中 + 留边**——取世界包围盒，按视口可用区域
//! （扣除四周留边）计算单一缩放系数，保证星图不因宽高比失真。
//! 世界坐标系 y 轴向上；像素坐标系 y 轴向下（图像习惯）；NDC y 轴向上。

/// 世界包围盒（力导向模拟坐标）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldBounds {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl WorldBounds {
    /// 由一组世界坐标计算包围盒；空集返回 `None`。
    pub fn of_positions(positions: impl Iterator<Item = (f32, f32)>) -> Option<Self> {
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        let mut any = false;
        for (x, y) in positions {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            any = true;
            min[0] = min[0].min(x);
            min[1] = min[1].min(y);
            max[0] = max[0].max(x);
            max[1] = max[1].max(y);
        }
        if !any {
            None
        } else {
            Some(Self { min, max })
        }
    }

    /// 分位数裁剪包围盒：每侧丢弃 `trim` 比例的最远坐标（如 0.10 = 各丢 10%），
    /// 使个别离群节点（布局未松弛时被弹飞的星体）不会把整幅星图压成一团，
    /// 离群点容许游出画面边缘。点数不足（`len × trim < 1`）时退化为完整包围盒。
    pub fn of_positions_trimmed(
        positions: impl Iterator<Item = (f32, f32)>,
        trim: f32,
    ) -> Option<Self> {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for (x, y) in positions {
            if x.is_finite() && y.is_finite() {
                xs.push(x);
                ys.push(y);
            }
        }
        if xs.is_empty() {
            return None;
        }
        // 每侧裁剪个数 = floor(点数 × trim)，比例上限 45%；不足 1 个则不裁剪
        let trim = ((xs.len() as f32) * trim.clamp(0.0, 0.45)).floor() as usize;
        if trim == 0 || xs.len() <= trim * 2 {
            return Some(Self {
                min: [
                    xs.iter().cloned().fold(f32::INFINITY, f32::min),
                    ys.iter().cloned().fold(f32::INFINITY, f32::min),
                ],
                max: [
                    xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                    ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                ],
            });
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let lo = trim;
        let hi = xs.len() - 1 - trim;
        Some(Self {
            min: [xs[lo], ys[lo]],
            max: [xs[hi], ys[hi]],
        })
    }

    /// 包围盒中心
    pub fn center(&self) -> [f32; 2] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
        ]
    }

    /// 包围盒宽高（至少一个极小量，避免后续除零）
    pub fn size(&self) -> [f32; 2] {
        [
            (self.max[0] - self.min[0]).max(1e-4),
            (self.max[1] - self.min[1]).max(1e-4),
        ]
    }
}

/// 一次适配结果：世界 → 像素的仿射变换参数
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewFit {
    /// 每世界单位对应的像素数
    pub scale: f32,
    /// 世界原点映射到的像素位置
    pub origin_px: [f32; 2],
    /// 视口尺寸（像素）
    pub viewport_px: [f32; 2],
}

/// 留边比例（视口短边的 6%）+ 基础留白（12 像素），保证边缘节点光晕不被裁切
pub fn padding_px(min_viewport_px: f32) -> f32 {
    if !min_viewport_px.is_finite() || min_viewport_px <= 0.0 {
        return 12.0;
    }
    12.0 + 0.06 * min_viewport_px
}

impl ViewFit {
    /// 计算适配：等比缩放居中；`bounds` 为 `None`（空图）时退化为原点居中、缩放 1。
    /// 缩放同时夹取在 `[1e-3, 1e4]`，防御极端包围盒导致的数值问题。
    pub fn compute(bounds: Option<WorldBounds>, viewport_px: [f32; 2], padding: f32) -> Self {
        let [w, h] = viewport_px;
        let (w_usable, h_usable) = ((w - 2.0 * padding).max(1.0), (h - 2.0 * padding).max(1.0));
        let (center, size) = match bounds {
            Some(b) => (b.center(), b.size()),
            None => ([0.0, 0.0], [1.0, 1.0]),
        };
        let scale = (w_usable / size[0])
            .min(h_usable / size[1])
            .clamp(1e-3, 1e4);
        let origin_px = [w * 0.5 - center[0] * scale, h * 0.5 - center[1] * scale];
        Self {
            scale,
            origin_px,
            viewport_px,
        }
    }

    /// 世界坐标 → 像素坐标（y 翻转：世界 +y → 屏幕 -y）
    pub fn world_to_pixel(&self, x: f32, y: f32) -> [f32; 2] {
        [
            self.origin_px[0] + x * self.scale,
            self.origin_px[1] - y * self.scale,
        ]
    }

    /// 像素坐标 → NDC（y 翻转：像素 y 向下 → NDC y 向上）
    pub fn pixel_to_ndc(&self, px: [f32; 2]) -> [f32; 2] {
        let [w, h] = self.viewport_px;
        [px[0] / w * 2.0 - 1.0, 1.0 - px[1] / h * 2.0]
    }

    /// 世界坐标 → NDC（组合便捷函数）
    pub fn world_to_ndc(&self, x: f32, y: f32) -> [f32; 2] {
        self.pixel_to_ndc(self.world_to_pixel(x, y))
    }

    /// NDC → 像素（`pixel_to_ndc` 的逆变换；供命中测试 / 拾取使用）
    pub fn ndc_to_pixel(&self, ndc: [f32; 2]) -> [f32; 2] {
        let [w, h] = self.viewport_px;
        [(ndc[0] + 1.0) * 0.5 * w, (1.0 - ndc[1]) * 0.5 * h]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: [f32; 2] = [1920.0, 1080.0];

    #[test]
    fn bounds_center_and_size() {
        let b = WorldBounds::of_positions([(0.0, 0.0), (10.0, -20.0), (-30.0, 5.0)].into_iter())
            .expect("非空必有包围盒");
        assert_eq!(b.min, [-30.0, -20.0]);
        assert_eq!(b.max, [10.0, 5.0]);
        assert_eq!(b.center(), [-10.0, -7.5]);
        assert_eq!(b.size(), [40.0, 25.0]);
        // 非有限坐标被忽略
        let b2 = WorldBounds::of_positions([(1.0, f32::NAN), (2.0, 3.0)].into_iter()).unwrap();
        assert_eq!(b2.min, [2.0, 3.0]);
        assert_eq!(b2.max, [2.0, 3.0]);
        assert!(WorldBounds::of_positions(std::iter::empty()).is_none());
    }

    #[test]
    fn trimmed_bounds_drop_outliers_per_axis() {
        // x：[-100, 0, 1, 2, 3, 4, 5, 1000]，trim 1/8 → 丢 -100 与 1000
        // y：同序 [-100, 0, 1, 2, 3, 4, 5, 1000] → 丢两端
        let pts: Vec<(f32, f32)> = [-100.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 1000.0]
            .into_iter()
            .map(|v| (v, v))
            .collect();
        let b = WorldBounds::of_positions_trimmed(pts.into_iter(), 0.125).unwrap();
        assert_eq!(b.min, [0.0, 0.0]);
        assert_eq!(b.max, [5.0, 5.0]);

        // 点数不足（len × trim < 1）时退化为完整包围盒
        let few: Vec<(f32, f32)> = (0..4).map(|i| (i as f32, i as f32)).collect();
        let full = WorldBounds::of_positions_trimmed(few.into_iter(), 0.2).unwrap();
        assert_eq!(full.min, [0.0, 0.0]);
        assert_eq!(full.max, [3.0, 3.0]);

        // 空集 / trim=0 行为
        assert!(WorldBounds::of_positions_trimmed(std::iter::empty(), 0.1).is_none());
        let two = vec![(1.0, 2.0), (3.0, 4.0)];
        let raw = WorldBounds::of_positions_trimmed(two.into_iter(), 0.0).unwrap();
        assert_eq!((raw.min, raw.max), ([1.0, 2.0], [3.0, 4.0]));
    }

    #[test]
    fn fit_centers_and_preserves_aspect() {
        // 4:1 的世界横条放进 16:9 视口：宽度受限 → scale = 可用宽/世界宽
        let bounds = WorldBounds {
            min: [-200.0, -25.0],
            max: [200.0, 25.0],
        };
        let fit = ViewFit::compute(Some(bounds), VP, 0.0);
        // 世界中心 (0,0) 应映射到视口中心
        let center_px = fit.world_to_pixel(0.0, 0.0);
        assert!((center_px[0] - VP[0] / 2.0).abs() < 1e-3);
        assert!((center_px[1] - VP[1] / 2.0).abs() < 1e-3);
        // 等比：世界 dx=dy 时像素距离相等
        let p1 = fit.world_to_pixel(0.0, 0.0);
        let p2 = fit.world_to_pixel(10.0, 0.0);
        let p3 = fit.world_to_pixel(0.0, 10.0);
        let dx = (p2[0] - p1[0]).abs();
        let dy = (p1[1] - p3[1]).abs();
        assert!((dx - dy).abs() < 1e-3, "等比缩放被破坏：{dx} vs {dy}");
        // 包围盒两端点都落在可用宽度内（padding=0）
        let left = fit.world_to_pixel(-200.0, 0.0)[0];
        let right = fit.world_to_pixel(200.0, 0.0)[0];
        assert!((left - 0.0).abs() < 1.0 && (right - VP[0]).abs() < 1.0);
    }

    #[test]
    fn world_y_up_pixel_y_down() {
        let fit = ViewFit::compute(None, VP, 0.0);
        // 世界 +y → 像素 y 更小（向下为正的屏幕系）
        let below = fit.world_to_pixel(0.0, 10.0)[1];
        let above = fit.world_to_pixel(0.0, -10.0)[1];
        assert!(below < above);
        // NDC：世界 +y → ndc y 更大
        let ndc = fit.world_to_ndc(0.0, 10.0);
        assert!(ndc[1] > 0.0);
        // 视口左上角像素 (0,0) → NDC (-1, 1)
        let tl = fit.pixel_to_ndc([0.0, 0.0]);
        assert!((tl[0] + 1.0).abs() < 1e-5 && (tl[1] - 1.0).abs() < 1e-5);
        // 右下角 → (1, -1)
        let br = fit.pixel_to_ndc(VP);
        assert!((br[0] - 1.0).abs() < 1e-4 && (br[1] + 1.0).abs() < 1e-4);
    }

    #[test]
    fn ndc_pixel_roundtrip() {
        let fit = ViewFit::compute(
            Some(WorldBounds {
                min: [-10.0, -5.0],
                max: [30.0, 40.0],
            }),
            VP,
            40.0,
        );
        for px in [
            [0.0, 0.0],
            [VP[0], VP[1]],
            [VP[0] / 3.0, VP[1] * 0.75],
            [123.5, 456.25],
        ] {
            let back = fit.ndc_to_pixel(fit.pixel_to_ndc(px));
            assert!(
                (back[0] - px[0]).abs() < 1e-3,
                "x 往返失败：{px:?} → {back:?}"
            );
            assert!(
                (back[1] - px[1]).abs() < 1e-3,
                "y 往返失败：{px:?} → {back:?}"
            );
        }
    }

    #[test]
    fn degenerate_bounds_do_not_explode() {
        // 单点 / 空图：不产生 NaN，原点居中
        let single = WorldBounds {
            min: [5.0, 5.0],
            max: [5.0, 5.0],
        };
        let fit = ViewFit::compute(Some(single), VP, 30.0);
        let p = fit.world_to_pixel(5.0, 5.0);
        assert!(p[0].is_finite() && p[1].is_finite());
        assert!((p[0] - VP[0] / 2.0).abs() < 1e-3);

        let empty = ViewFit::compute(None, VP, 30.0);
        let p0 = empty.world_to_ndc(0.0, 0.0);
        assert!((p0[0]).abs() < 1e-5 && (p0[1]).abs() < 1e-5);

        // 极小视口也不至于非法
        let tiny = ViewFit::compute(Some(single), [1.0, 1.0], 0.0);
        assert!(tiny.world_to_pixel(0.0, 0.0)[0].is_finite());
    }

    #[test]
    fn padding_keeps_nodes_off_edges() {
        let bounds = WorldBounds {
            min: [-100.0, -100.0],
            max: [100.0, 100.0],
        };
        let pad = padding_px(VP[1].min(VP[0]));
        let fit = ViewFit::compute(Some(bounds), VP, pad);
        // 正方形世界放 16:9 视口：高度受限，上下各留 pad
        let top = fit.world_to_pixel(0.0, 100.0)[1];
        let bottom = fit.world_to_pixel(0.0, -100.0)[1];
        assert!(top >= pad - 1.0, "上边缘节点被裁切：{top} < {pad}");
        assert!(bottom <= VP[1] - pad + 1.0, "下边缘节点被裁切：{bottom}");
    }
}
