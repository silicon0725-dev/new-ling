//! # ling-render
//!
//! 「灵」成长星图的 wgpu 离屏渲染器（纯 GPU 侧，不引入任何 UI 框架）。
//!
//! 职责：消费 [`ling_core::starmap`] 力导向模拟产出的节点 / 边数据
//! （特质节点、记忆节点、归属 / 因果连线），用 WGSL 着色器绘制：
//! - 星节点：核心圆盘 + **克制的白色发光光晕**（指数衰减，缓慢呼吸）；
//! - 动态连线：柔边光带 + 沿线流光（因果边流速更快）；
//! - 背景微粒子：确定性分布、缓慢漂移与闪烁的星尘。
//!
//! 本 crate **始终离屏渲染到纹理**（不创建窗口 surface），再由
//! [`StarmapRenderer::read_pixels_rgba8`] 取回紧密排列的 RGBA8 像素缓冲，
//! 上层（如 slint 应用）可自行将其包成图像显示——因此本 crate 无需也不允许依赖 slint。
//!
//! ## 典型用法
//!
//! ```no_run
//! use ling_render::{StarSnapshot, StarmapRenderer};
//!
//! // 1. 创建渲染器（需要可用的 GPU 适配器；纯逻辑环境会返回 Err）
//! let mut renderer = StarmapRenderer::new(1280, 720)?;
//!
//! // 2. 装载节点 / 边快照（可用 ling-core 领域数据一键构建，见 StarSnapshot::from_character_memory）
//! renderer.load_snapshot(StarSnapshot::from_character_memory(/* &Character */ todo!(), /* &[MemoryEvent] */ &[], /* &[MemoryArc] */ &[]));
//!
//! // 3. 每帧：推进布局一步并绘制一帧，随后取回 RGBA8 像素交给上层成像
//! loop {
//!     renderer.step_and_draw();
//!     let rgba: Vec<u8> = renderer.read_pixels_rgba8()?;
//!     # let _ = rgba; break; // 演示用，实际循环交给 UI 帧驱动
//! }
//! # Ok::<(), ling_render::RenderError>(())
//! ```
//!
//! ## 约定
//! - 像素缓冲为 **自上而下、行优先** 的 RGBA8（sRGB 编码，预乘与否由混合模式决定为加法合成后的颜色）；
//! - 布局推进使用固定步长（60 FPS 对应 1/60 秒动画时间），调用方按帧驱动即可；
//! - 所有可见参数（颜色、半径、厚度）均为纯函数换算，见 [`visual`]、[`geometry`]、[`instance`] 模块内联单测。

#![forbid(unsafe_code)]

pub mod geometry;
pub mod instance;
pub mod renderer;
pub mod snapshot;
pub mod visual;

mod block_on;

pub use geometry::{ViewFit, WorldBounds};
pub use renderer::{RenderError, StarmapRenderer};
pub use snapshot::{EdgeKind, EdgeVisual, NodeKind, NodeVisual, StarSnapshot};
