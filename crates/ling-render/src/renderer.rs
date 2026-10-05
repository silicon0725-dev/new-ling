//! 成长星图离屏渲染器：wgpu 设备 / 管线管理与帧调度。
//!
//! 渲染目标始终是一张离屏 RGBA8(sRGB) 纹理（不创建窗口 surface，因此
//! 不需要 raw-window-handle / winit / slint），每帧流程：
//! 1. `Starmap::step()` 推动力导向布局一步（ling-core 纯逻辑模拟）；
//! 2. CPU 侧重算视口适配并组装节点 / 边实例字节（纯函数，见 geometry/instance/visual）；
//! 3. WGSL 三管线绘制：背景微粒子 → 动态连线 → 星节点（白色光晕）；
//! 4. 需要成像时用 [`StarmapRenderer::read_pixels_rgba8`] 拷回紧密 RGBA8 缓冲。

use std::borrow::Cow;
use std::time::Duration;

use ling_core::starmap::Starmap;

use crate::block_on::block_on;
use crate::geometry::{padding_px, ViewFit, WorldBounds};
use crate::instance::{
    repack_rgba_rows, EdgeInstance, NodeInstance, EDGE_INSTANCE_SIZE, NODE_INSTANCE_SIZE,
    PARTICLE_INSTANCE_SIZE,
};
use crate::snapshot::{EdgeKind, StarSnapshot};
use crate::visual::{edge_color, edge_thickness_px, node_palette, node_radius_px, BACKGROUND};

/// 离屏目标纹理格式（读回字节即 sRGB 编码的 RGBA8，可直接交给上层图像 API）
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// 每帧动画推进的固定步长（秒）——按 60 FPS 帧驱动设计
pub const FRAME_DT: f32 = 1.0 / 60.0;

/// 三份着色器都拼接共享头（uniform 结构 + 坐标工具函数）
const NODE_SHADER: &str = concat!(
    include_str!("shader/common.wgsl"),
    include_str!("shader/node.wgsl")
);
const EDGE_SHADER: &str = concat!(
    include_str!("shader/common.wgsl"),
    include_str!("shader/edge.wgsl")
);
const PARTICLE_SHADER: &str = concat!(
    include_str!("shader/common.wgsl"),
    include_str!("shader/particle.wgsl")
);

/// 渲染初始化 / 读回失败的原因
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// 找不到可用 GPU 适配器（纯逻辑 / 无显卡环境）
    NoAdapter(String),
    /// 适配器拒绝创建设备
    DeviceRequest(String),
    /// `device.poll` 失败（设备丢失等）
    PollFailed(String),
    /// 暂存缓冲映射超时
    MapTimeout,
    /// 暂存缓冲映射失败
    MapFailed(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::NoAdapter(why) => write!(f, "找不到可用 GPU 适配器：{why}"),
            RenderError::DeviceRequest(why) => write!(f, "创建 GPU 设备失败：{why}"),
            RenderError::PollFailed(why) => write!(f, "GPU 轮询失败：{why}"),
            RenderError::MapTimeout => write!(f, "GPU 像素读回超时"),
            RenderError::MapFailed(why) => write!(f, "GPU 缓冲映射失败：{why}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// 尺寸防御：任何 0 / 超大值都夹取到 1..=8192
fn clamp_size(width: u32, height: u32) -> (u32, u32) {
    (width.clamp(1, 8192), height.clamp(1, 8192))
}

/// 纹理拷贝到缓冲要求的每行对齐（256 的倍数）
fn padded_row_bytes(width: u32) -> usize {
    let tight = (width as usize) * 4;
    tight.div_ceil(256) * 256
}

/// 加法混合：颜色通道直接叠加（发光叠加），alpha 保持清屏值 1
const BLEND_ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// 成长星图渲染器（离屏；线程不安全——按帧从单一线程驱动）
pub struct StarmapRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,
    /// 离屏颜色目标（RENDER_ATTACHMENT | COPY_SRC）
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    /// 每行对齐后的字节数（读回布局）
    padded_row: usize,
    /// 读回暂存缓冲（MAP_READ | COPY_DST，按需创建）
    staging: Option<wgpu::Buffer>,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    node_pipe: wgpu::RenderPipeline,
    edge_pipe: wgpu::RenderPipeline,
    particle_pipe: wgpu::RenderPipeline,
    node_buf: Option<wgpu::Buffer>,
    node_capacity: u64,
    edge_buf: Option<wgpu::Buffer>,
    edge_capacity: u64,
    particle_buf: Option<wgpu::Buffer>,
    particle_capacity: u64,
    /// 已上传的粒子实例数（绘制时使用，区别于字节容量）
    particle_count: u32,
    /// 视口变化后粒子需重传
    particles_dirty: bool,
    /// 力导向模拟（星体下标与 snapshot.nodes 严格对齐）
    sim: Starmap,
    /// 已装载（且已规整）的快照
    snapshot: StarSnapshot,
    /// 边列表：模拟星体下标对 (a, b) + 视觉属性
    edge_links: Vec<(usize, usize, EdgeKind, f32)>,
    /// 当前动画时间（秒）
    time_s: f32,
    /// 全局光晕强度（0.1..3.0）
    glow: f32,
    frame_index: u64,
}

impl StarmapRenderer {
    /// 创建渲染器：请求高性能 GPU 适配器并构建全部管线。
    ///
    /// 需要真实 GPU 环境；无适配器时返回 [`RenderError::NoAdapter`]。
    /// `width` / `height` 为离屏目标初始尺寸（会被夹取到 1..=8192）。
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ling-render 设备"),
            ..Default::default()
        }))
        .map_err(|e| RenderError::DeviceRequest(e.to_string()))?;

        // —— 全局 uniform（32 字节，布局见 shader/common.wgsl）——
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ling-render 全局 uniform 布局"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ling-render 全局 uniform"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ling-render 全局 uniform 绑定组"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &globals,
                    offset: 0,
                    size: None,
                }),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ling-render 管线布局"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });

        // —— 三条渲染管线（共用 uniform 布局与混合模式）——
        let make_module = |src: &'static str, label: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(src)),
            })
        };
        let node_module = make_module(NODE_SHADER, "ling-render 节点着色器");
        let edge_module = make_module(EDGE_SHADER, "ling-render 连线着色器");
        let particle_module = make_module(PARTICLE_SHADER, "ling-render 粒子着色器");

        let node_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: NODE_INSTANCE_SIZE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 3,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 48,
                    shader_location: 4,
                },
            ],
        })];
        let edge_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: EDGE_INSTANCE_SIZE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 16,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 3,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 48,
                    shader_location: 4,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 52,
                    shader_location: 5,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 56,
                    shader_location: 6,
                },
            ],
        })];
        let particle_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: PARTICLE_INSTANCE_SIZE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 32,
                    shader_location: 3,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 36,
                    shader_location: 4,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 40,
                    shader_location: 5,
                },
            ],
        })];

        let make_pipeline = |label: &str,
                             module: &wgpu::ShaderModule,
                             layout: &[Option<wgpu::VertexBufferLayout<'static>>]|
         -> wgpu::RenderPipeline {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: layout,
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET_FORMAT,
                        blend: Some(BLEND_ADDITIVE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let node_pipe = make_pipeline("ling-render 节点管线", &node_module, &node_layout);
        let edge_pipe = make_pipeline("ling-render 连线管线", &edge_module, &edge_layout);
        let particle_pipe =
            make_pipeline("ling-render 粒子管线", &particle_module, &particle_layout);

        let (width, height) = clamp_size(width, height);
        let (target, target_view) = create_target(&device, width, height);
        Ok(Self {
            device,
            queue,
            width,
            height,
            target,
            target_view,
            padded_row: padded_row_bytes(width),
            staging: None,
            globals,
            globals_bg,
            node_pipe,
            edge_pipe,
            particle_pipe,
            node_buf: None,
            node_capacity: 0,
            edge_buf: None,
            edge_capacity: 0,
            particle_buf: None,
            particle_capacity: 0,
            particle_count: 0,
            particles_dirty: true,
            sim: Starmap::new(&[], &[]),
            snapshot: StarSnapshot::default(),
            edge_links: Vec::new(),
            time_s: 0.0,
            glow: 1.0,
            frame_index: 0,
        })
    }

    /// 装载节点 / 边快照：内部会做规整（夹取 / 重排句柄 / 丢弃悬空边），
    /// 并以确定性圆环初始化重建力导向模拟。旧布局状态被整体替换。
    pub fn load_snapshot(&mut self, snapshot: StarSnapshot) {
        let clean = snapshot.sanitized();
        let ids: Vec<i64> = clean.nodes.iter().map(|n| n.id).collect();
        let mut sim = Starmap::new(&ids, &[]);
        self.edge_links = clean
            .edges
            .iter()
            .map(|e| (e.from as usize, e.to as usize, e.kind, e.strength))
            .collect();
        // 归属边短、因果边长：让结构聚拢、时间链舒展
        for &(a, b, kind, _) in &self.edge_links {
            let rest_length = match kind {
                EdgeKind::Ownership => 110.0,
                EdgeKind::Causal => 150.0,
            };
            sim.push_link(a, b, rest_length);
        }
        self.sim = sim;
        self.snapshot = clean;
    }

    /// 每帧推进布局一步并绘制一帧；返回本步最大位移（可用作收敛判据，
    /// 与 [`ling_core::starmap::Starmap::step`] 一致）。
    pub fn step_and_draw(&mut self) -> f64 {
        let max_displacement = self.sim.step();
        let viewport = [self.width as f32, self.height as f32];
        let min_dim = viewport[0].min(viewport[1]);
        // 分位数裁剪包围盒：离群星体（布局未松弛时被弹飞）不把整幅星图压小
        let bounds = WorldBounds::of_positions_trimmed(
            self.sim.stars.iter().map(|s| (s.x as f32, s.y as f32)),
            0.10,
        );
        let fit = ViewFit::compute(bounds, viewport, padding_px(min_dim));

        // —— 组装节点实例（CPU 纯逻辑；星体下标与快照节点一一对应）——
        let nodes: Vec<NodeInstance> = self
            .sim
            .stars
            .iter()
            .zip(self.snapshot.nodes.iter())
            .enumerate()
            .map(|(i, (star, visual))| {
                let palette = node_palette(visual.kind, visual.weight);
                NodeInstance {
                    center_px: fit.world_to_pixel(star.x as f32, star.y as f32),
                    radius_px: node_radius_px(visual.kind, visual.weight, min_dim),
                    core_rgba: palette.core,
                    halo_rgba: palette.halo,
                    // 黄金比例散列相位，避免所有节点同频呼吸
                    pulse: (i as f32 * 0.618_034).fract(),
                }
            })
            .collect();
        let node_bytes =
            crate::instance::encode_batch(&nodes, NODE_INSTANCE_SIZE, |n| n.encode().to_vec());

        // —— 组装边实例 ——
        let edges: Vec<EdgeInstance> = self
            .edge_links
            .iter()
            .enumerate()
            .map(|(j, &(a, b, kind, strength))| {
                let (sa, sb) = (&self.sim.stars[a], &self.sim.stars[b]);
                EdgeInstance {
                    a_px: fit.world_to_pixel(sa.x as f32, sa.y as f32),
                    b_px: fit.world_to_pixel(sb.x as f32, sb.y as f32),
                    thickness_px: edge_thickness_px(kind, strength, min_dim),
                    rgba: edge_color(kind, strength),
                    flow_phase: (j as f32 * 0.37).fract(),
                    kind: match kind {
                        EdgeKind::Ownership => 0.0,
                        EdgeKind::Causal => 1.0,
                    },
                    strength,
                }
            })
            .collect();
        let edge_bytes =
            crate::instance::encode_batch(&edges, EDGE_INSTANCE_SIZE, |e| e.encode().to_vec());

        // —— 粒子实例（仅视口相关，脏时才重传）——
        let particle_bytes;
        if self.particles_dirty {
            let particles = crate::instance::build_particles(viewport, 0x4C_49_4E_47);
            particle_bytes = Some(crate::instance::encode_batch(
                &particles,
                PARTICLE_INSTANCE_SIZE,
                |p| p.encode().to_vec(),
            ));
        } else {
            particle_bytes = None;
        }

        // —— 上传 ——
        self.upload_nodes(&node_bytes);
        self.upload_edges(&edge_bytes);
        if let Some(bytes) = &particle_bytes {
            self.upload_particles(bytes);
            self.particles_dirty = false;
        }
        let globals_bytes = encode_globals(viewport, self.time_s, self.glow);
        self.queue.write_buffer(&self.globals, 0, &globals_bytes);

        // —— 绘制（微粒子 → 连线 → 节点；加法混合下顺序不影响叠加结果）——
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ling-render 帧编码器"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ling-render 星图离屏通道"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: BACKGROUND[0] as f64,
                            g: BACKGROUND[1] as f64,
                            b: BACKGROUND[2] as f64,
                            a: BACKGROUND[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let particle_instances = self.particle_count;
            if particle_instances > 0 {
                pass.set_pipeline(&self.particle_pipe);
                pass.set_bind_group(0, &self.globals_bg, &[]);
                pass.set_vertex_buffer(0, self.particle_buf.as_ref().unwrap().slice(..));
                pass.draw(0..6, 0..particle_instances);
            }
            let edge_count = self.edge_links.len() as u32;
            if edge_count > 0 {
                pass.set_pipeline(&self.edge_pipe);
                pass.set_bind_group(0, &self.globals_bg, &[]);
                pass.set_vertex_buffer(0, self.edge_buf.as_ref().unwrap().slice(..));
                pass.draw(0..6, 0..edge_count);
            }
            let node_count = self.snapshot.nodes.len() as u32;
            if node_count > 0 {
                pass.set_pipeline(&self.node_pipe);
                pass.set_bind_group(0, &self.globals_bg, &[]);
                pass.set_vertex_buffer(0, self.node_buf.as_ref().unwrap().slice(..));
                pass.draw(0..6, 0..node_count);
            }
        }
        self.queue.submit([encoder.finish()]);

        self.frame_index += 1;
        self.time_s += FRAME_DT;
        max_displacement
    }

    /// 窗口尺寸自适应：重建离屏目标与读回布局，粒子场随新视口重生成。
    /// 尺寸被夹取到 1..=8192；与当前相同则为幂等空操作。
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = clamp_size(width, height);
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        let (target, view) = create_target(&self.device, width, height);
        self.target = target;
        self.target_view = view;
        self.padded_row = padded_row_bytes(width);
        self.staging = None; // 旧暂存缓冲尺寸失效，下次读回重建
        self.particles_dirty = true;
    }

    /// 离屏读回：把当前帧从 GPU 纹理拷回 CPU，返回**紧密排列、自上而下、行优先**
    /// 的 RGBA8 缓冲（长度 `width × 4 × height`，字节为 sRGB 编码），
    /// 上层可直接包成图像（如 `slint::Image` —— 由上层完成，本 crate 不依赖 slint）。
    ///
    /// 该调用包含一次 GPU 同步等待；每帧调用一次是可接受的开销。
    pub fn read_pixels_rgba8(&mut self) -> Result<Vec<u8>, RenderError> {
        let needed = (self.padded_row * self.height as usize) as u64;
        let staging = match &self.staging {
            Some(buf) if buf.size() == needed => buf.clone(),
            _ => {
                let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("ling-render 读回暂存"),
                    size: needed,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.staging = Some(buf.clone());
                buf
            }
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ling-render 读回编码器"),
            });
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row as u32),
                    rows_per_image: None,
                },
                buffer: &staging,
            },
            self.target.size(),
        );
        let submission = self.queue.submit([encoder.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(5)),
            })
            .map_err(|e| RenderError::PollFailed(e.to_string()))?;
        let mapped = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| RenderError::MapTimeout)?
            .map_err(|e| RenderError::MapFailed(format!("{e:?}")))?;
        let _ = mapped;

        let tight = {
            let view = staging
                .slice(..)
                .get_mapped_range()
                .map_err(|e| RenderError::MapFailed(format!("{e:?}")))?;
            repack_rgba_rows(&view, self.width, self.height, self.padded_row)
        };
        staging.unmap();
        Ok(tight)
    }

    /// 设置全局光晕强度（夹取 0.1..3.0，默认 1.0）
    pub fn set_glow_intensity(&mut self, glow: f32) {
        self.glow = if glow.is_finite() {
            glow.clamp(0.1, 3.0)
        } else {
            1.0
        };
    }

    /// 当前光晕强度
    pub fn glow_intensity(&self) -> f32 {
        self.glow
    }

    /// 离屏目标宽（像素）
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 离屏目标高（像素）
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 已装载节点数
    pub fn node_count(&self) -> usize {
        self.snapshot.nodes.len()
    }

    /// 已装载边数
    pub fn edge_count(&self) -> usize {
        self.edge_links.len()
    }

    /// 当前动画时间（秒）
    pub fn time_s(&self) -> f32 {
        self.time_s
    }

    /// 已绘制帧数（含装载后未推进布局的帧）
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// 只读访问内部力导向模拟（可用于 settle / 查询坐标等 ling-core 能力）
    pub fn simulation(&self) -> &Starmap {
        &self.sim
    }

    /// 节点实例缓冲容量不足则重建（预留一倍余量），再整块上传
    fn upload_nodes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.node_capacity < bytes.len() as u64 {
            self.node_capacity = grow_to(bytes.len() as u64);
            self.node_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ling-render 节点实例"),
                size: self.node_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        self.queue
            .write_buffer(self.node_buf.as_ref().unwrap(), 0, bytes);
    }

    /// 边实例缓冲同上
    fn upload_edges(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.edge_capacity < bytes.len() as u64 {
            self.edge_capacity = grow_to(bytes.len() as u64);
            self.edge_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ling-render 边实例"),
                size: self.edge_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        self.queue
            .write_buffer(self.edge_buf.as_ref().unwrap(), 0, bytes);
    }

    /// 粒子实例缓冲同上；同时记录精确实例数供绘制使用
    fn upload_particles(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            self.particle_count = 0;
            return;
        }
        if self.particle_capacity < bytes.len() as u64 {
            self.particle_capacity = grow_to(bytes.len() as u64);
            self.particle_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ling-render 粒子实例"),
                size: self.particle_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        self.particle_count = (bytes.len() as u32) / (PARTICLE_INSTANCE_SIZE as u32);
        self.queue
            .write_buffer(self.particle_buf.as_ref().unwrap(), 0, bytes);
    }
}

/// 容量增长策略：至少 4 KiB，翻倍并 16 字节对齐
fn grow_to(needed: u64) -> u64 {
    let doubled = needed.max(4096);
    (doubled + 15) & !15
}

/// 创建离屏颜色目标与其视图
fn create_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ling-render 离屏颜色目标"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// 编码 32 字节全局 uniform（布局与 shader/common.wgsl 的 Globals 一致）
fn encode_globals(viewport: [f32; 2], time: f32, glow: f32) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let mut put = |offset: usize, v: f32| {
        buf[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
    };
    put(0, viewport[0]);
    put(4, viewport[1]);
    put(16, time);
    put(20, glow);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_clamping() {
        assert_eq!(clamp_size(0, 0), (1, 1));
        assert_eq!(clamp_size(100, 200), (100, 200));
        assert_eq!(clamp_size(99_999, 7), (8192, 7));
    }

    #[test]
    fn row_padding_to_256() {
        assert_eq!(padded_row_bytes(1), 256);
        assert_eq!(padded_row_bytes(64), 256); // 64*4=256 恰好对齐
        assert_eq!(padded_row_bytes(65), 512);
        assert_eq!(padded_row_bytes(1920), 7680 + 0); // 1920*4=7680 是 256 的倍数
        assert_eq!(padded_row_bytes(1000), 4096); // 4000 → 4096
    }

    #[test]
    fn globals_encoding_layout() {
        let bytes = encode_globals([1920.0, 1080.0], 1.5, 0.8);
        let read = |off: usize| {
            let mut b = [0u8; 4];
            b.copy_from_slice(&bytes[off..off + 4]);
            f32::from_le_bytes(b)
        };
        assert_eq!(read(0), 1920.0);
        assert_eq!(read(4), 1080.0);
        assert_eq!(read(16), 1.5);
        assert_eq!(read(20), 0.8);
        // 填充区（8..16 与 24..32）保持零
        assert!(bytes[8..16].iter().all(|&b| b == 0));
        assert!(bytes[24..].iter().all(|&b| b == 0));
    }

    #[test]
    fn capacity_growth_is_aligned_and_generous() {
        assert_eq!(grow_to(0), 4096);
        assert_eq!(grow_to(100), 4096);
        assert_eq!(grow_to(5000), 5008); // 5000+15 & !15
        assert!(grow_to(60000) >= 60000);
        assert!(grow_to(1 << 20) % 16 == 0);
    }

    /// GPU 冒烟测试：真实设备上跑一帧并读回像素。
    /// 默认跳过（CI / 无显卡环境不可用）：`cargo test -p ling-render -- --ignored`
    #[test]
    #[ignore = "需要真实 GPU 设备，手动执行验证"]
    fn offscreen_smoke_frame() {
        use crate::snapshot::{NodeKind, NodeVisual};
        let mut renderer = StarmapRenderer::new(320, 240).expect("应有可用 GPU");
        renderer.load_snapshot(StarSnapshot {
            nodes: vec![
                NodeVisual {
                    id: 0,
                    kind: NodeKind::Character,
                    weight: 1.0,
                },
                NodeVisual {
                    id: 1,
                    kind: NodeKind::Trait,
                    weight: 0.8,
                },
                NodeVisual {
                    id: 2,
                    kind: NodeKind::MemoryEvent,
                    weight: 0.5,
                },
            ],
            edges: vec![
                crate::snapshot::EdgeVisual {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::Ownership,
                    strength: 0.8,
                },
                crate::snapshot::EdgeVisual {
                    from: 1,
                    to: 2,
                    kind: EdgeKind::Causal,
                    strength: 0.5,
                },
            ],
        });
        let _ = renderer.step_and_draw();
        let pixels = renderer.read_pixels_rgba8().expect("读回应成功");
        assert_eq!(pixels.len(), 320 * 240 * 4);
        // resize 后尺寸与缓冲同步变化
        renderer.resize(128, 96);
        let _ = renderer.step_and_draw();
        let pixels = renderer.read_pixels_rgba8().expect("读回应成功");
        assert_eq!(pixels.len(), 128 * 96 * 4);
    }
}
