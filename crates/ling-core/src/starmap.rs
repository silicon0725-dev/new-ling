//! 星图力导向布局：纯逻辑模拟（确定性圆环初始化，无随机数，便于测试与复现）。
//!
//! 力模型：
//! - 任意两颗星之间存在库仑斥力（与距离平方成反比）；
//! - 每条连边是一根弹簧（胡克定律，自然长度 rest_length）；
//! - 速度按阻尼系数衰减，单步速度限幅防止数值爆炸；
//! - 收敛判据：单步最大位移小于阈值 epsilon。

/// 一颗星（节点）
#[derive(Debug, Clone)]
pub struct Star {
    pub id: i64,
    pub x: f64,
    pub y: f64,
    vx: f64,
    vy: f64,
    /// 是否固定（不参与移动，可作锚点）
    pub fixed: bool,
}

/// 一条连边（弹簧）
#[derive(Debug, Clone)]
pub struct StarLink {
    pub a: usize,
    pub b: usize,
    /// 弹簧自然长度
    pub rest_length: f64,
}

/// 收敛报告
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettleReport {
    /// 实际迭代步数
    pub iterations: usize,
    /// 是否在最大步数内收敛
    pub converged: bool,
    /// 收敛时的单步最大位移（未收敛则为最后一步的值）
    pub final_max_displacement: f64,
}

/// 默认弹簧自然长度
pub const DEFAULT_REST_LENGTH: f64 = 40.0;

/// 星图模拟器
pub struct Starmap {
    pub stars: Vec<Star>,
    pub links: Vec<StarLink>,
    /// 库仑斥力系数
    pub repulsion: f64,
    /// 弹簧劲度系数
    pub spring_k: f64,
    /// 速度阻尼（每步速度乘以该系数）
    pub damping: f64,
    /// 积分步长
    pub dt: f64,
    /// 单步最大速度限幅
    pub max_velocity: f64,
}

impl Starmap {
    /// 新建星图：节点沿圆环确定性初始化（半径随节点数缩放），连边用默认自然长度
    pub fn new(ids: &[i64], links: &[(usize, usize)]) -> Self {
        Self::with_parameters(ids, links, 2000.0, 0.5, 0.85, 1.0, 40.0)
    }

    /// 全参数构造（斥力 / 弹簧系数 / 阻尼 / 步长 / 限幅）
    pub fn with_parameters(
        ids: &[i64],
        links: &[(usize, usize)],
        repulsion: f64,
        spring_k: f64,
        damping: f64,
        dt: f64,
        max_velocity: f64,
    ) -> Self {
        let n = ids.len();
        let radius = 40.0 * (n as f64).sqrt().max(1.0);
        let stars = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                // n 为 0 时不会产出任何节点，这里只需防除零
                let angle = i as f64 * std::f64::consts::TAU / n.max(1) as f64;
                Star {
                    id: *id,
                    x: radius * angle.cos(),
                    y: radius * angle.sin(),
                    vx: 0.0,
                    vy: 0.0,
                    fixed: false,
                }
            })
            .collect();
        let links = links
            .iter()
            .map(|&(a, b)| StarLink {
                a,
                b,
                rest_length: DEFAULT_REST_LENGTH,
            })
            .collect();
        Self {
            stars,
            links,
            repulsion,
            spring_k,
            damping,
            dt,
            max_velocity,
        }
    }

    /// 追加一条自定义自然长度的连边
    pub fn push_link(&mut self, a: usize, b: usize, rest_length: f64) {
        self.links.push(StarLink { a, b, rest_length });
    }

    /// 执行一步模拟，返回本步最大位移（用作收敛判据）
    pub fn step(&mut self) -> f64 {
        let n = self.stars.len();
        let mut fx = vec![0.0; n];
        let mut fy = vec![0.0; n];

        // 两两库仑斥力
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = self.stars[j].x - self.stars[i].x;
                let dy = self.stars[j].y - self.stars[i].y;
                // 重合保护，避免除零
                let mut dist2 = dx * dx + dy * dy;
                if dist2 < 1e-6 {
                    dist2 = 1e-6;
                }
                let dist = dist2.sqrt();
                let force = self.repulsion / dist2;
                let ux = dx / dist;
                let uy = dy / dist;
                fx[i] -= force * ux;
                fy[i] -= force * uy;
                fx[j] += force * ux;
                fy[j] += force * uy;
            }
        }

        // 弹簧引力（沿连边，可正可负）
        for link in &self.links {
            if link.a == link.b || link.a >= n || link.b >= n {
                continue;
            }
            let dx = self.stars[link.b].x - self.stars[link.a].x;
            let dy = self.stars[link.b].y - self.stars[link.a].y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < 1e-9 {
                continue;
            }
            let force = self.spring_k * (dist - link.rest_length);
            let ux = dx / dist;
            let uy = dy / dist;
            fx[link.a] += force * ux;
            fy[link.a] += force * uy;
            fx[link.b] -= force * ux;
            fy[link.b] -= force * uy;
        }

        // 阻尼积分 + 速度限幅
        let mut max_displacement = 0.0;
        for i in 0..n {
            let star = &mut self.stars[i];
            if star.fixed {
                star.vx = 0.0;
                star.vy = 0.0;
                continue;
            }
            star.vx = (star.vx + fx[i] * self.dt) * self.damping;
            star.vy = (star.vy + fy[i] * self.dt) * self.damping;
            let speed = (star.vx * star.vx + star.vy * star.vy).sqrt();
            if speed > self.max_velocity {
                let scale = self.max_velocity / speed;
                star.vx *= scale;
                star.vy *= scale;
            }
            star.x += star.vx * self.dt;
            star.y += star.vy * self.dt;
            let displacement = (star.vx * star.vx + star.vy * star.vy).sqrt() * self.dt;
            if displacement > max_displacement {
                max_displacement = displacement;
            }
        }
        max_displacement
    }

    /// 迭代至收敛（单步最大位移 < epsilon）或达到最大步数
    pub fn settle(&mut self, max_iterations: usize, epsilon: f64) -> SettleReport {
        let mut last = f64::INFINITY;
        for iteration in 1..=max_iterations {
            last = self.step();
            if last < epsilon {
                return SettleReport {
                    iterations: iteration,
                    converged: true,
                    final_max_displacement: last,
                };
            }
        }
        SettleReport {
            iterations: max_iterations,
            converged: false,
            final_max_displacement: last,
        }
    }

    /// 输出 (id, x, y)
    pub fn positions(&self) -> Vec<(i64, f64, f64)> {
        self.stars.iter().map(|s| (s.id, s.x, s.y)).collect()
    }

    /// 两颗星的当前距离（下标越界会 panic，调用方保证合法）
    pub fn distance(&self, a: usize, b: usize) -> f64 {
        let (s, t) = (&self.stars[a], &self.stars[b]);
        ((s.x - t.x).powi(2) + (s.y - t.y).powi(2)).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pair_settles_near_rest_length() {
        let mut map = Starmap::new(&[1, 2], &[(0, 1)]);
        let report = map.settle(20_000, 0.01);
        assert!(report.converged, "应当收敛：{report:?}");
        let d = map.distance(0, 1);
        // 弹簧与斥力的平衡点会略大于自然长度，取宽松区间
        assert!((20.0..=120.0).contains(&d), "平衡间距异常：{d}");
        assert!(map.positions().iter().all(|(_, x, y)| x.is_finite() && y.is_finite()));
    }

    #[test]
    fn clustered_graph_converges_with_short_links() {
        // 双三角 + 一条桥
        let links = [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)];
        let mut map = Starmap::new(&[1, 2, 3, 4, 5, 6], &links);
        let report = map.settle(30_000, 0.01);
        assert!(report.converged, "应当收敛：{report:?}");
        for (_, x, y) in map.positions() {
            assert!(x.is_finite() && y.is_finite(), "坐标必须有限");
        }
        // 收敛后：连边平均间距应小于非连边平均间距（图结构被保留）
        let avg_linked: f64 =
            links.iter().map(|&(a, b)| map.distance(a, b)).sum::<f64>() / links.len() as f64;
        let mut total = 0.0;
        let mut count = 0.0;
        for a in 0..6 {
            for b in (a + 1)..6 {
                let linked = links
                    .iter()
                    .any(|&(x, y)| (x == a && y == b) || (x == b && y == a));
                if !linked {
                    total += map.distance(a, b);
                    count += 1.0;
                }
            }
        }
        let avg_unlinked = total / count;
        assert!(
            avg_linked < avg_unlinked,
            "连边平均间距（{avg_linked}）应小于非连边（{avg_unlinked}）"
        );
    }

    #[test]
    fn fixed_star_never_moves() {
        let mut map = Starmap::new(&[1, 2, 3], &[(0, 1), (1, 2)]);
        map.stars[1].x = 10.0;
        map.stars[1].y = -5.0;
        map.stars[1].fixed = true;
        map.settle(5_000, 0.01);
        assert_eq!((map.stars[1].x, map.stars[1].y), (10.0, -5.0));
    }
}
