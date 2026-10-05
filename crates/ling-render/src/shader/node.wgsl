// 星节点：实例化四边形 → 核心圆盘 + 克制的白色光晕（加法混合）。
// 实例布局见 instance.rs 的 NODE_INSTANCE_SIZE 注释（偏移严格一致）。

struct NodeInstance {
    @location(0) center_px: vec2<f32>, // 星心像素坐标
    @location(1) radius_px: f32,       // 光晕外沿半径（像素）
    @location(2) core_rgba: vec4<f32>, // 核心颜色（预乘）
    @location(3) halo_rgba: vec4<f32>, // 光晕颜色（预乘）
    @location(4) pulse: f32,           // 呼吸相位种子
};

struct NodeOut {
    @builtin(position) position: vec4<f32>,
    @location(0) q: vec2<f32>,        // 单位四边形坐标（仿射量，插值无损）
    @location(1) pulse: f32,
    @location(2) core_rgba: vec4<f32>,
    @location(3) halo_rgba: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, inst: NodeInstance) -> NodeOut {
    // 单位四边形（两个三角形拼成，覆盖 [-1,1]^2）
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let q = quad[vertex_index];
    var out: NodeOut;
    out.position = vec4<f32>(px_to_ndc(inst.center_px + q * inst.radius_px), 0.0, 1.0);
    out.q = q;
    out.pulse = inst.pulse;
    out.core_rgba = inst.core_rgba;
    out.halo_rgba = inst.halo_rgba;
    return out;
}

@fragment
fn fs_main(in: NodeOut) -> @location(0) vec4<f32> {
    // 半径归一化距离必须在片元内重算：length 不是仿射量，插值会失真
    let r = length(in.q);
    // 核心圆盘：r < 0.40 全亮，到 0.55 柔和淡出
    let core = 1.0 - smoothstep(0.40, 0.55, r);
    // 光晕：指数快速衰减 —— 「克制」的关键（1 倍半径处约剩 5%）
    let halo = exp(-3.0 * r);
    // 呼吸：±10% 的缓慢起伏，让星图有生命感但不喧闹
    let breathe = 0.90 + 0.10 * sin(globals.time * 1.6 + in.pulse * 6.28318);
    let rgb = (in.core_rgba.rgb * core + in.halo_rgba.rgb * halo * breathe) * globals.glow;
    let a = in.core_rgba.a * core + in.halo_rgba.a * halo * breathe;
    return vec4<f32>(rgb, a);
}
