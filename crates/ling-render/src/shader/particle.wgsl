// 背景微粒子：确定性分布的星尘，缓慢漂移 + 闪烁，克制的暗白。

struct ParticleInstance {
    @location(0) base_px: vec2<f32>, // 基准像素坐标
    @location(1) radius_px: f32,     // 半径（像素）
    @location(2) rgba: vec4<f32>,    // 颜色（预乘，本就极暗）
    @location(3) drift: f32,         // 漂移速率
    @location(4) seed: f32,          // 随机种子（相位）
    @location(5) twinkle: f32,       // 闪烁频率
};

struct ParticleOut {
    @builtin(position) position: vec4<f32>,
    @location(0) rgba: vec4<f32>,
    @location(1) twinkle_seed: vec2<f32>,
    @location(2) q2: vec2<f32>, // 单位四边形坐标（仿射量，插值无损）
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, inst: ParticleInstance) -> ParticleOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let q = quad[vertex_index];
    // 缓慢圆游漂移：半径 4..8 像素，速率由 drift 控制
    let t = globals.time * inst.drift;
    let drift_offset = vec2<f32>(
        sin(t + inst.seed * 6.28318),
        cos(t * 0.8 + inst.seed * 10.9),
    ) * (4.0 + 4.0 * inst.drift);
    let p = inst.base_px + drift_offset + q * inst.radius_px;
    var out: ParticleOut;
    out.position = vec4<f32>(px_to_ndc(p), 0.0, 1.0);
    out.rgba = inst.rgba;
    out.twinkle_seed = vec2<f32>(inst.twinkle, inst.seed);
    out.q2 = q;
    return out;
}

@fragment
fn fs_main(in: ParticleOut) -> @location(0) vec4<f32> {
    // 半径归一化距离必须在片元内重算：length 不是仿射量，插值会失真
    let r = length(in.q2);
    // 高斯光斑衰减
    let falloff = exp(-3.0 * r * r);
    // 闪烁：65% 基准 + 35% 起伏
    let twinkle = 0.65 + 0.35 * sin(globals.time * in.twinkle_seed.x * 2.0 + in.twinkle_seed.y * 6.28318);
    let a = in.rgba.a * falloff * twinkle * 0.85 * globals.glow;
    return vec4<f32>(in.rgba.rgb * falloff * twinkle * 0.85 * globals.glow, a);
}
