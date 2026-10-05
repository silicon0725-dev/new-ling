//! GPU 顶点 / 实例数据的字节组装。
//!
//! 不引入 bytemuck：所有字段用 `f32::to_le_bytes` 按固定偏移手工编码，
//! 偏移量遵循 WGSL 结构体的自然对齐规则（vec2 对齐 8、vec4 对齐 16），
//! 与 renderer 侧声明的顶点属性表、shader 内的结构体一一对应。
//!
//! 同时提供离屏读回后的**行重排**（纹理拷贝要求每行 256 字节对齐，
//! 上层图像需要紧密排列的行）与确定性伪随机数（背景粒子，无 rand 依赖）。

/// 节点实例：stride 64 字节
///
/// 布局（偏移）：
/// - 0: `center_px: vec2<f32>`（像素坐标）
/// - 8: `radius_px: f32`（光晕外沿半径）
/// - 16: `core_rgba: vec4<f32>`（预乘线性色）
/// - 32: `halo_rgba: vec4<f32>`
/// - 48: `pulse: f32`（呼吸相位种子）
/// - 52..64: 填充
pub const NODE_INSTANCE_SIZE: u64 = 64;

/// 边实例：stride 64 字节
///
/// 布局（偏移）：
/// - 0: `a_px: vec2<f32>` / 8: `b_px: vec2<f32>`（两端像素坐标）
/// - 16: `thickness_px: f32`（半厚度）
/// - 32: `rgba: vec4<f32>`
/// - 48: `flow_phase: f32` / 52: `kind: f32`（0 归属 1 因果）/ 56: `strength: f32`
pub const EDGE_INSTANCE_SIZE: u64 = 64;

/// 粒子实例：stride 48 字节
///
/// 布局（偏移）：
/// - 0: `base_px: vec2<f32>` / 8: `radius_px: f32`
/// - 16: `rgba: vec4<f32>`
/// - 32: `drift: f32`（漂移速度）/ 36: `seed: f32` / 40: `twinkle: f32`（闪烁频率）
pub const PARTICLE_INSTANCE_SIZE: u64 = 48;

/// 在 `offset` 处写入一个 `f32`（小端）
fn put_f32(buf: &mut [u8], offset: usize, v: f32) {
    buf[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
}

/// 在 `offset` 处连续写入 `n` 个 `f32`
fn put_f32_slice(buf: &mut [u8], offset: usize, vs: &[f32]) {
    for (i, v) in vs.iter().enumerate() {
        put_f32(buf, offset + i * 4, *v);
    }
}

/// 从 `offset` 处读回一个 `f32`（仅测试用，用于验证往返编码）
#[cfg(test)]
fn get_f32(buf: &[u8], offset: usize) -> f32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&buf[offset..offset + 4]);
    f32::from_le_bytes(b)
}

/// 节点实例数据（CPU 侧表达，编码前）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeInstance {
    pub center_px: [f32; 2],
    pub radius_px: f32,
    pub core_rgba: [f32; 4],
    pub halo_rgba: [f32; 4],
    pub pulse: f32,
}

impl NodeInstance {
    /// 编码为 64 字节（GPU 布局见 [`NODE_INSTANCE_SIZE`] 注释）
    pub fn encode(&self) -> [u8; NODE_INSTANCE_SIZE as usize] {
        let mut buf = [0u8; NODE_INSTANCE_SIZE as usize];
        put_f32_slice(&mut buf, 0, &self.center_px);
        put_f32(&mut buf, 8, self.radius_px);
        put_f32_slice(&mut buf, 16, &self.core_rgba);
        put_f32_slice(&mut buf, 32, &self.halo_rgba);
        put_f32(&mut buf, 48, self.pulse);
        buf
    }
}

/// 边实例数据（CPU 侧表达，编码前）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeInstance {
    pub a_px: [f32; 2],
    pub b_px: [f32; 2],
    pub thickness_px: f32,
    pub rgba: [f32; 4],
    pub flow_phase: f32,
    /// 0.0 = 归属，1.0 = 因果（shader 内做插值开关）
    pub kind: f32,
    pub strength: f32,
}

impl EdgeInstance {
    /// 编码为 64 字节
    pub fn encode(&self) -> [u8; EDGE_INSTANCE_SIZE as usize] {
        let mut buf = [0u8; EDGE_INSTANCE_SIZE as usize];
        put_f32_slice(&mut buf, 0, &self.a_px);
        put_f32_slice(&mut buf, 8, &self.b_px);
        put_f32(&mut buf, 16, self.thickness_px);
        put_f32_slice(&mut buf, 32, &self.rgba);
        put_f32(&mut buf, 48, self.flow_phase);
        put_f32(&mut buf, 52, self.kind);
        put_f32(&mut buf, 56, self.strength);
        buf
    }
}

/// 粒子实例数据（CPU 侧表达，编码前）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleInstance {
    pub base_px: [f32; 2],
    pub radius_px: f32,
    pub rgba: [f32; 4],
    pub drift: f32,
    pub seed: f32,
    pub twinkle: f32,
}

impl ParticleInstance {
    /// 编码为 48 字节
    pub fn encode(&self) -> [u8; PARTICLE_INSTANCE_SIZE as usize] {
        let mut buf = [0u8; PARTICLE_INSTANCE_SIZE as usize];
        put_f32_slice(&mut buf, 0, &self.base_px);
        put_f32(&mut buf, 8, self.radius_px);
        put_f32_slice(&mut buf, 16, &self.rgba);
        put_f32(&mut buf, 32, self.drift);
        put_f32(&mut buf, 36, self.seed);
        put_f32(&mut buf, 40, self.twinkle);
        buf
    }
}

/// 把一批实例连续编码成一个顶点缓冲的字节序列
pub fn encode_batch<T>(
    items: &[T],
    stride: u64,
    mut encode_one: impl FnMut(&T) -> Vec<u8>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(items.len() * stride as usize);
    for item in items {
        out.extend_from_slice(&encode_one(item));
    }
    out
}

/// splitmix64：确定性伪随机（无 rand 依赖，跨平台一致）
#[derive(Debug, Clone)]
pub struct DetRng(pub u64);

impl DetRng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    /// 下一个 u64
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 下一个 f32 ∈ [0, 1)
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// 背景微粒子数量：按视口面积缩放（约每 2.2 万像素 1 颗），夹取 24..160。
pub fn particle_count(viewport_px: [f32; 2]) -> usize {
    let area = viewport_px[0].max(0.0) * viewport_px[1].max(0.0);
    if !area.is_finite() {
        return 24;
    }
    ((area / 22_000.0) as usize).clamp(24, 160)
}

/// 生成一套背景微粒子（确定性：同一视口尺寸 + 同一种子 → 同一结果）。
/// 颜色取极暗的白/蓝白，alpha 0.10..0.35，保持「微」粒子的克制观感。
pub fn build_particles(viewport_px: [f32; 2], seed: u64) -> Vec<ParticleInstance> {
    let count = particle_count(viewport_px);
    let min_dim = viewport_px[0].min(viewport_px[1]);
    let mut rng = DetRng::new(
        seed ^ (viewport_px[0].to_bits() as u64) ^ ((viewport_px[1].to_bits() as u64) << 32),
    );
    (0..count)
        .map(|_| {
            let x = rng.next_f32() * viewport_px[0];
            let y = rng.next_f32() * viewport_px[1];
            let r = 0.5 + rng.next_f32() * 0.5;
            let b = 0.7 + rng.next_f32() * 0.3;
            let alpha = 0.16 + rng.next_f32() * 0.30;
            ParticleInstance {
                base_px: [x, y],
                radius_px: (1.2 + 1.6 * rng.next_f32()) * crate::visual::ui_scale(min_dim),
                rgba: [
                    r * alpha,
                    r * b * alpha * 0.5 + alpha * 0.5,
                    b * alpha,
                    alpha,
                ],
                drift: 0.4 + rng.next_f32() * 1.2,
                seed: rng.next_f32(),
                twinkle: 0.4 + rng.next_f32() * 1.4,
            }
        })
        .collect()
}

/// 离屏读回后去掉每行的 256 字节对齐填充，重排成紧密的行优先 RGBA8。
///
/// `padded` 长度必须为 `padded_row * height`，返回长度 `width * 4 * height`。
pub fn repack_rgba_rows(padded: &[u8], width: u32, height: u32, padded_row: usize) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let tight_row = width * 4;
    let mut out = Vec::with_capacity(tight_row * height);
    for row in 0..height {
        let start = row * padded_row;
        let end = start + tight_row;
        if end > padded.len() {
            break;
        }
        out.extend_from_slice(&padded[start..end]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_instance_layout_offsets() {
        let n = NodeInstance {
            center_px: [11.5, -22.25],
            radius_px: 33.0,
            core_rgba: [0.1, 0.2, 0.3, 0.4],
            halo_rgba: [0.5, 0.6, 0.7, 0.8],
            pulse: 0.25,
        };
        let b = n.encode();
        assert_eq!(b.len(), NODE_INSTANCE_SIZE as usize);
        assert_eq!(get_f32(&b, 0), 11.5);
        assert_eq!(get_f32(&b, 4), -22.25);
        assert_eq!(get_f32(&b, 8), 33.0);
        for (k, v) in n.core_rgba.iter().enumerate() {
            assert_eq!(get_f32(&b, 16 + k * 4), *v);
        }
        for (k, v) in n.halo_rgba.iter().enumerate() {
            assert_eq!(get_f32(&b, 32 + k * 4), *v);
        }
        assert_eq!(get_f32(&b, 48), 0.25);
        // 尾部填充区全零
        assert!(b[52..].iter().all(|&x| x == 0));
    }

    #[test]
    fn edge_instance_layout_offsets() {
        let e = EdgeInstance {
            a_px: [1.0, 2.0],
            b_px: [3.0, 4.0],
            thickness_px: 1.5,
            rgba: [0.25; 4],
            flow_phase: 0.75,
            kind: 1.0,
            strength: 0.5,
        };
        let b = e.encode();
        assert_eq!(b.len(), EDGE_INSTANCE_SIZE as usize);
        assert_eq!(get_f32(&b, 0), 1.0);
        assert_eq!(get_f32(&b, 4), 2.0);
        assert_eq!(get_f32(&b, 8), 3.0);
        assert_eq!(get_f32(&b, 12), 4.0);
        assert_eq!(get_f32(&b, 16), 1.5);
        // 20..32 为对齐填充
        assert!(b[20..32].iter().all(|&x| x == 0));
        for k in 0..4 {
            assert_eq!(get_f32(&b, 32 + k * 4), 0.25);
        }
        assert_eq!(get_f32(&b, 48), 0.75);
        assert_eq!(get_f32(&b, 52), 1.0);
        assert_eq!(get_f32(&b, 56), 0.5);
        // 60..64 尾部填充
        assert!(b[60..].iter().all(|&x| x == 0));
    }

    #[test]
    fn particle_instance_layout_offsets() {
        let p = ParticleInstance {
            base_px: [7.0, 8.0],
            radius_px: 1.25,
            rgba: [0.2, 0.3, 0.4, 0.5],
            drift: 0.9,
            seed: 0.1,
            twinkle: 1.1,
        };
        let b = p.encode();
        assert_eq!(b.len(), PARTICLE_INSTANCE_SIZE as usize);
        assert_eq!(get_f32(&b, 0), 7.0);
        assert_eq!(get_f32(&b, 4), 8.0);
        assert_eq!(get_f32(&b, 8), 1.25);
        assert!(b[12..16].iter().all(|&x| x == 0)); // 12..16 对齐填充
        for k in 0..4 {
            assert_eq!(get_f32(&b, 16 + k * 4), [0.2, 0.3, 0.4, 0.5][k]);
        }
        assert_eq!(get_f32(&b, 32), 0.9);
        assert_eq!(get_f32(&b, 36), 0.1);
        assert_eq!(get_f32(&b, 40), 1.1);
        assert!(b[44..].iter().all(|&x| x == 0));
    }

    #[test]
    fn encode_batch_packs_contiguously() {
        let nodes = vec![
            NodeInstance {
                center_px: [1.0, 1.0],
                radius_px: 5.0,
                core_rgba: [1.0, 1.0, 1.0, 1.0],
                halo_rgba: [0.0; 4],
                pulse: 0.0,
            },
            NodeInstance {
                center_px: [2.0, 2.0],
                radius_px: 6.0,
                core_rgba: [1.0, 1.0, 1.0, 1.0],
                halo_rgba: [0.0; 4],
                pulse: 0.5,
            },
        ];
        let bytes = encode_batch(&nodes, NODE_INSTANCE_SIZE, |n| n.encode().to_vec());
        assert_eq!(bytes.len(), 2 * NODE_INSTANCE_SIZE as usize);
        // 第二个实例的首字段紧跟第一个实例结尾
        assert_eq!(get_f32(&bytes, NODE_INSTANCE_SIZE as usize), 2.0);
    }

    #[test]
    fn det_rng_is_deterministic_and_in_range() {
        let mut a = DetRng::new(42);
        let mut b = DetRng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = DetRng::new(43);
        assert_ne!(
            DetRng::new(42).next_u64(),
            c.next_u64(),
            "不同种子应产出不同序列"
        );
        let mut d = DetRng::new(7);
        for _ in 0..1000 {
            let v = d.next_f32();
            assert!((0.0..1.0).contains(&v), "next_f32 越界：{v}");
        }
    }

    #[test]
    fn particle_count_scales_with_area() {
        assert_eq!(particle_count([1920.0, 1080.0]), 94); // 1920*1080/22000 ≈ 94
        assert_eq!(particle_count([100.0, 100.0]), 24); // 下限
        assert_eq!(particle_count([8000.0, 8000.0]), 160); // 上限
        assert_eq!(particle_count([f32::NAN, 0.0]), 24); // 非法输入兜底
    }

    #[test]
    fn particles_are_deterministic_and_inside_viewport() {
        let vp = [800.0, 600.0];
        let p1 = build_particles(vp, 99);
        let p2 = build_particles(vp, 99);
        assert_eq!(p1, p2, "同种子同视口必须逐位一致");
        let p3 = build_particles(vp, 100);
        assert_ne!(p1, p3, "不同种子的粒子场应不同");
        for p in &p1 {
            assert!(
                p.base_px[0] >= 0.0 && p.base_px[0] < vp[0],
                "粒子 x 出界：{:?}",
                p.base_px
            );
            assert!(
                p.base_px[1] >= 0.0 && p.base_px[1] < vp[1],
                "粒子 y 出界：{:?}",
                p.base_px
            );
            assert!(p.radius_px.is_finite() && p.radius_px > 0.0);
            assert!(p
                .rgba
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
        }
        // 视口尺寸参与种子：resize 后粒子场变化
        let p4 = build_particles([801.0, 600.0], 99);
        assert_ne!(p1, p4);
    }

    #[test]
    fn repack_drops_row_padding() {
        // 构造 4x2 像素、padded_row=32 的读回数据（每行前 16 字节有效）
        let mut padded = vec![0xAAu8; 32 * 2];
        for row in 0..2 {
            for col in 0..4 {
                for ch in 0..4 {
                    padded[row * 32 + col * 4 + ch] = (row * 16 + col * 4 + ch) as u8;
                }
            }
        }
        let tight = repack_rgba_rows(&padded, 4, 2, 32);
        assert_eq!(tight.len(), 4 * 4 * 2);
        assert_eq!(&tight[0..4], &[0, 1, 2, 3]);
        assert_eq!(&tight[16..20], &[16, 17, 18, 19]);
        // 输入过短时安全截断（不 panic）
        let short = repack_rgba_rows(&padded[..40], 4, 2, 32);
        assert_eq!(short.len(), 16); // 只有第一行完整
    }
}
