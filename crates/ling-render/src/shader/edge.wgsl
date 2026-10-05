// 动态连线：沿两端点拉伸的实例化四边形 → 柔边光带 + 沿线流光。
// 归属边慢而稳（结构感），因果边快而亮（时间感）。

struct EdgeInstance {
    @location(0) a_px: vec2<f32>,      // 起点像素坐标
    @location(1) b_px: vec2<f32>,      // 终点像素坐标
    @location(2) thickness_px: f32,    // 半厚度（像素）
    @location(3) rgba: vec4<f32>,      // 颜色（预乘）
    @location(4) flow_phase: f32,      // 流光相位（避免所有边同频）
    @location(5) kind: f32,            // 0 = 归属，1 = 因果
    @location(6) strength: f32,        // 强度
};

struct EdgeOut {
    @builtin(position) position: vec4<f32>,
    @location(0) st: vec2<f32>,        // x: 沿线 0..1，y: 横向 -1..1
    @location(1) rgba: vec4<f32>,
    @location(2) flow: f32,
    @location(3) kind_strength: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, inst: EdgeInstance) -> EdgeOut {
    // x ∈ {0,1} 为沿线参数，y ∈ {-1,1} 为横向参数
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let q = quad[vertex_index];
    let ab = inst.b_px - inst.a_px;
    let len = max(length(ab), 0.0001);
    let dir = ab / len;
    let nrm = vec2<f32>(-dir.y, dir.x); // 垂直方向
    let p = inst.a_px + ab * q.x + nrm * (q.y * inst.thickness_px);
    var out: EdgeOut;
    out.position = vec4<f32>(px_to_ndc(p), 0.0, 1.0);
    out.st = q;
    out.rgba = inst.rgba;
    out.flow = inst.flow_phase;
    out.kind_strength = vec2<f32>(inst.kind, inst.strength);
    return out;
}

@fragment
fn fs_main(in: EdgeOut) -> @location(0) vec4<f32> {
    let s = clamp(in.st.x, 0.0, 1.0);
    let t = in.st.y;
    // 横向柔边：抛物线截面，中心最亮
    let lateral = clamp(1.0 - t * t, 0.0, 1.0);
    // 端点淡出：避免连线生硬抵住星点
    let ends = s * (1.0 - s) * 4.0;
    // 流光：沿线移动的微亮脉冲；因果边更快（0.9）更明显
    let speed = mix(0.35, 0.9, in.kind_strength.x);
    let wave = 0.6 + 0.4 * sin((s * 2.0 - globals.time * speed + in.flow) * 6.28318);
    let shimmer = mix(1.0, wave, 0.45);
    let a = in.rgba.a * lateral * ends * in.kind_strength.y * shimmer * globals.glow;
    return vec4<f32>(in.rgba.rgb * lateral * ends * in.kind_strength.y * shimmer * globals.glow, a);
}
