// 全局 uniform：与 Rust 侧 renderer.rs 写入的 32 字节布局严格对应（小端、显式填充）。
struct Globals {
    viewport: vec2<f32>, // 渲染目标尺寸（像素）
    _pad0: vec2<f32>,    // 8 字节对齐填充
    time: f32,           // 累计动画时间（秒，每帧 +1/60）
    glow: f32,           // 全局光晕强度系数（默认 1.0）
    _pad1: vec2<f32>,    // 尾部填充
};

@group(0) @binding(0) var<uniform> globals: Globals;

// 像素坐标 → 裁剪空间（NDC）：像素 y 向下、NDC y 向上，故 y 轴翻转
fn px_to_ndc(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        p.x / globals.viewport.x * 2.0 - 1.0,
        1.0 - p.y / globals.viewport.y * 2.0,
    );
}
