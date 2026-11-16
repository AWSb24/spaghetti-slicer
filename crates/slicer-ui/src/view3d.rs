//! The 3D view: the bed, the model as a shaded mesh and the toolpaths up
//! to a layer, seen through an orbit camera and drawn with OpenGL in an
//! egui paint callback.
//!
//! Vertex buffers are made once per bed, model and slice (see
//! [`slicer_ui::scene`]); a frame only sets a few uniforms and issues a
//! handful of draw calls. The maths lives in [`slicer_ui::camera`] and
//! [`slicer_ui::scene`] with tests; this file is the untested GL glue.

// Every OpenGL call through glow is `unsafe`. The calls here only pass
// buffers and handles this module created itself, on the thread and
// context egui paints with.
#![allow(unsafe_code)]

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align2, FontId, PointerButton, Sense, Vec2};
use eframe::egui_glow::{self, ShaderVersion};
use eframe::glow::{self, HasContext};
use slicer_fdm::Toolpaths;
use slicer_ui::camera::{Mat4, OrbitCamera, multiply};
use slicer_ui::plate::{Model, Placement};
use slicer_ui::scene::{
    Lines, Triangles, bed_grid, bed_plate, mesh_triangles, model_matrix, toolpath_lines,
};

/// Brightness of the layers below the chosen one, which is drawn at full
/// colour.
const BELOW_DIM: f32 = 0.45;

/// What the 3D view shows in this frame.
pub struct Scene<'a> {
    /// Bed size in millimetres.
    pub bed: [f64; 2],
    /// Build height in millimetres, for the first camera position.
    pub height: f64,
    /// A number that changes whenever models are added or removed.
    pub serial: u64,
    /// Every model on the plate, where it sits, whether it is selected and
    /// whether it fits the printer.
    pub models: Vec<(&'a Model, Placement, bool, bool)>,
    /// Show the toolpaths up to and including this layer instead of the
    /// model.
    pub layer: Option<usize>,
}

/// State of the 3D view: camera and GPU buffers.
#[derive(Default)]
pub struct View3d {
    /// `None` until the first frame, which fits it to the bed.
    camera: Option<OrbitCamera>,
    /// Bed and theme the bed buffers were made for.
    bed_sent: Option<([f64; 2], bool)>,
    /// Plate the model buffers were made for.
    models_sent: Option<u64>,
    gpu: Arc<Mutex<Gpu>>,
}

impl View3d {
    /// Hands the toolpaths of a new slice to the GPU on the next frame.
    pub fn set_toolpaths(&mut self, toolpaths: &Toolpaths) {
        let lines = toolpath_lines(toolpaths);
        if let Ok(mut gpu) = self.gpu.lock() {
            gpu.pending_paths = Some(lines);
        }
    }

    /// Draws the scene into the rest of `ui` and handles the mouse.
    pub fn show(&mut self, ui: &mut egui::Ui, scene: &Scene) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;
        let dark = ui.visuals().dark_mode;
        if self.bed_sent.is_some_and(|(bed, _)| bed != scene.bed) || response.double_clicked() {
            self.camera = None;
        }
        let camera = self
            .camera
            .get_or_insert_with(|| OrbitCamera::fit(scene.bed, scene.height));

        let delta = response.drag_delta();
        let shift = ui.input(|i| i.modifiers.shift);
        if response.dragged_by(PointerButton::Primary) && !shift {
            camera.rotate(delta.x, delta.y);
        } else if response.dragged() {
            camera.pan(delta.x, delta.y, rect.height());
        }
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            camera.zoom((scroll / 200.0).exp() * zoom);
        }

        let frame = Frame {
            view_projection: camera.view_projection(rect.width() / rect.height().max(1.0)),
            light: camera.light_direction(),
            models: if scene.layer.is_none() {
                scene
                    .models
                    .iter()
                    .map(|(model, placement, selected, fits)| {
                        let color = match (fits, selected) {
                            (false, _) => [230, 60, 60],
                            (true, true) => [120, 175, 255],
                            (true, false) => [76, 141, 255],
                        };
                        (model_matrix(model, placement), rgb(color))
                    })
                    .collect()
            } else {
                Vec::new()
            },
            layer: scene.layer,
            plate: rgb(if dark { [48, 48, 52] } else { [222, 222, 226] }),
        };
        self.send(scene, dark);

        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        let gpu = Arc::clone(&self.gpu);
        painter.add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                if let Ok(mut gpu) = gpu.lock() {
                    gpu.paint(painter.gl(), &frame);
                }
            })),
        });
        let error = self.gpu.lock().ok().and_then(|gpu| gpu.error.clone());
        let (text, color) = match error {
            Some(error) => (
                format!("3D view unavailable: {error}"),
                ui.visuals().error_fg_color,
            ),
            None => (
                "Drag to turn · right-drag or Shift+drag to pan · scroll to zoom · double-click to fit"
                    .to_owned(),
                ui.visuals().weak_text_color(),
            ),
        };
        painter.text(
            rect.left_bottom() + Vec2::new(8.0, -8.0),
            Align2::LEFT_BOTTOM,
            text,
            FontId::proportional(12.0),
            color,
        );
    }

    /// Builds vertex data for a changed bed or model.
    fn send(&mut self, scene: &Scene, dark: bool) {
        let Ok(mut gpu) = self.gpu.lock() else {
            return;
        };
        if self.bed_sent != Some((scene.bed, dark)) {
            self.bed_sent = Some((scene.bed, dark));
            let (grid, border) = if dark {
                ([70, 70, 76, 255], [150, 150, 156, 255])
            } else {
                ([190, 190, 196, 255], [110, 110, 116, 255])
            };
            gpu.pending_bed = Some((bed_plate(scene.bed), bed_grid(scene.bed, grid, border)));
        }
        if self.models_sent != Some(scene.serial) {
            self.models_sent = Some(scene.serial);
            gpu.pending_models = Some(
                scene
                    .models
                    .iter()
                    .map(|(model, ..)| mesh_triangles(&model.mesh))
                    .collect(),
            );
        }
    }
}

fn rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| f32::from(v) / 255.0)
}

/// Everything a frame needs from the app, copied into the paint callback.
struct Frame {
    view_projection: Mat4,
    light: [f32; 3],
    /// Model matrix and colour of every model shown, in plate order.
    models: Vec<(Mat4, [f32; 3])>,
    layer: Option<usize>,
    plate: [f32; 3],
}

/// GPU objects, made and used only inside the paint callback where the GL
/// context is current. Data waits in the `pending_` fields until then.
#[derive(Default)]
struct Gpu {
    programs: Option<Programs>,
    /// Why nothing can be drawn, once something failed.
    error: Option<String>,
    plate: Option<Buffers>,
    grid: Option<Buffers>,
    models: Vec<Buffers>,
    paths: Option<Buffers>,
    pending_bed: Option<(Triangles, Lines)>,
    pending_models: Option<Vec<Triangles>>,
    pending_paths: Option<Lines>,
}

/// A vertex array with its buffers.
struct Buffers {
    vao: glow::VertexArray,
    buffers: Vec<glow::Buffer>,
    /// Vertex count, and for lines the end of each layer.
    lines: Lines,
    count: i32,
}

struct Program {
    program: glow::Program,
    mvp: Option<glow::UniformLocation>,
    model: Option<glow::UniformLocation>,
    color: Option<glow::UniformLocation>,
    light: Option<glow::UniformLocation>,
    dim: Option<glow::UniformLocation>,
}

struct Programs {
    shaded: Program,
    lines: Program,
}

const SHADED_VERTEX: &str = "
uniform mat4 u_mvp;
uniform mat4 u_model;
ATTRIBUTE vec3 a_pos;
ATTRIBUTE vec3 a_normal;
VARYING vec3 v_normal;
void main() {
    v_normal = (u_model * vec4(a_normal, 0.0)).xyz;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
";

const SHADED_FRAGMENT: &str = "
uniform vec3 u_color;
uniform vec3 u_light;
VARYING vec3 v_normal;
void main() {
    float light = abs(dot(normalize(v_normal), u_light));
    FRAG_COLOR = vec4(u_color * (0.3 + 0.7 * light), 1.0);
}
";

const LINES_VERTEX: &str = "
uniform mat4 u_mvp;
uniform float u_dim;
ATTRIBUTE vec3 a_pos;
ATTRIBUTE vec4 a_color;
VARYING vec4 v_color;
void main() {
    v_color = vec4(a_color.rgb * u_dim, 1.0);
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
";

const LINES_FRAGMENT: &str = "
VARYING vec4 v_color;
void main() {
    FRAG_COLOR = v_color;
}
";

/// Lines that make one shader source work on GLSL 1.20 to 4.x and ES.
fn shader_header(version: ShaderVersion, fragment: bool) -> String {
    let mut header = version.version_declaration().to_owned();
    if version.is_embedded() && fragment {
        header.push_str("precision mediump float;\n");
    }
    header.push_str(match (version.is_new_shader_interface(), fragment) {
        (true, false) => "#define ATTRIBUTE in\n#define VARYING out\n",
        (true, true) => "#define VARYING in\nout vec4 frag_color;\n#define FRAG_COLOR frag_color\n",
        (false, _) => {
            "#define ATTRIBUTE attribute\n#define VARYING varying\n#define FRAG_COLOR gl_FragColor\n"
        }
    });
    header
}

impl Program {
    /// Compiles and links; attribute 0 is the position, 1 the normal or
    /// colour.
    fn new(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<Self, String> {
        let version = ShaderVersion::get(gl);
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, source, is_fragment) in [
                (glow::VERTEX_SHADER, vertex, false),
                (glow::FRAGMENT_SHADER, fragment, true),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &(shader_header(version, is_fragment) + source));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(gl.get_shader_info_log(shader));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.bind_attrib_location(program, 0, "a_pos");
            gl.bind_attrib_location(program, 1, "a_normal");
            gl.bind_attrib_location(program, 1, "a_color");
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            let uniform = |name: &str| gl.get_uniform_location(program, name);
            Ok(Self {
                program,
                mvp: uniform("u_mvp"),
                model: uniform("u_model"),
                color: uniform("u_color"),
                light: uniform("u_light"),
                dim: uniform("u_dim"),
            })
        }
    }
}

/// Positions as bytes for a vertex buffer.
fn f32_bytes(values: &[[f32; 3]]) -> Vec<u8> {
    values
        .as_flattened()
        .iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect()
}

impl Buffers {
    /// Uploads vertex attributes: (bytes, components, type, normalized).
    fn new(
        gl: &glow::Context,
        attributes: &[(&[u8], i32, u32, bool)],
        count: usize,
        lines: Lines,
    ) -> Result<Self, String> {
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let mut buffers = Vec::new();
            for (index, &(bytes, size, kind, normalized)) in attributes.iter().enumerate() {
                let buffer = gl.create_buffer()?;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
                gl.enable_vertex_attrib_array(index as u32);
                gl.vertex_attrib_pointer_f32(index as u32, size, kind, normalized, 0, 0);
                buffers.push(buffer);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(Self {
                vao,
                buffers,
                lines,
                count: i32::try_from(count).map_err(|e| e.to_string())?,
            })
        }
    }

    fn triangles(gl: &glow::Context, triangles: &Triangles) -> Result<Self, String> {
        Self::new(
            gl,
            &[
                (&f32_bytes(&triangles.positions), 3, glow::FLOAT, false),
                (&f32_bytes(&triangles.normals), 3, glow::FLOAT, false),
            ],
            triangles.positions.len(),
            Lines::default(),
        )
    }

    fn lines(gl: &glow::Context, lines: Lines) -> Result<Self, String> {
        let buffers = Self::new(
            gl,
            &[
                (&f32_bytes(&lines.positions), 3, glow::FLOAT, false),
                (lines.colors.as_flattened(), 4, glow::UNSIGNED_BYTE, true),
            ],
            lines.positions.len(),
            Lines::default(),
        )?;
        // Only the layer ends are needed from here on.
        Ok(Self {
            lines: Lines {
                layer_ends: lines.layer_ends,
                ..Lines::default()
            },
            ..buffers
        })
    }

    fn delete(self, gl: &glow::Context) {
        unsafe {
            gl.delete_vertex_array(self.vao);
            for buffer in self.buffers {
                gl.delete_buffer(buffer);
            }
        }
    }

    fn draw(&self, gl: &glow::Context, mode: u32, range: std::ops::Range<usize>) {
        let (Ok(first), Ok(count)) = (
            i32::try_from(range.start),
            i32::try_from(range.end.saturating_sub(range.start)),
        ) else {
            return;
        };
        if count > 0 {
            unsafe {
                gl.bind_vertex_array(Some(self.vao));
                gl.draw_arrays(mode, first, count);
            }
        }
    }
}

/// Replaces `slot` with new buffers, deleting the old ones.
fn replace(
    gl: &glow::Context,
    slot: &mut Option<Buffers>,
    new: Result<Buffers, String>,
) -> Result<(), String> {
    if let Some(old) = slot.take() {
        old.delete(gl);
    }
    *slot = Some(new?);
    Ok(())
}

impl Gpu {
    fn paint(&mut self, gl: &glow::Context, frame: &Frame) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.prepare(gl) {
            self.error = Some(error);
            return;
        }
        let Some(programs) = &self.programs else {
            return;
        };
        let identity: Mat4 = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        unsafe {
            gl.depth_mask(true);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.disable(glow::BLEND);

            let shaded = &programs.shaded;
            gl.use_program(Some(shaded.program));
            gl.uniform_3_f32_slice(shaded.light.as_ref(), &frame.light);
            let solid = |buffers: Option<&Buffers>, model: &Mat4, color: &[f32; 3]| {
                if let Some(buffers) = buffers {
                    let mvp = multiply(&frame.view_projection, model);
                    gl.uniform_matrix_4_f32_slice(shaded.mvp.as_ref(), false, &mvp);
                    gl.uniform_matrix_4_f32_slice(shaded.model.as_ref(), false, model);
                    gl.uniform_3_f32_slice(shaded.color.as_ref(), color);
                    let count = usize::try_from(buffers.count).unwrap_or(0);
                    buffers.draw(gl, glow::TRIANGLES, 0..count);
                }
            };
            solid(self.plate.as_ref(), &identity, &frame.plate);
            for (buffers, (model, color)) in self.models.iter().zip(&frame.models) {
                solid(Some(buffers), model, color);
            }

            let lines = &programs.lines;
            gl.use_program(Some(lines.program));
            gl.uniform_matrix_4_f32_slice(lines.mvp.as_ref(), false, &frame.view_projection);
            gl.uniform_1_f32(lines.dim.as_ref(), 1.0);
            if let Some(grid) = &self.grid {
                let count = usize::try_from(grid.count).unwrap_or(0);
                grid.draw(gl, glow::LINES, 0..count);
            }
            if let (Some(layer), Some(paths)) = (frame.layer, &self.paths) {
                gl.uniform_1_f32(lines.dim.as_ref(), BELOW_DIM);
                paths.draw(gl, glow::LINES, paths.lines.below(layer));
                gl.uniform_1_f32(lines.dim.as_ref(), 1.0);
                paths.draw(gl, glow::LINES, paths.lines.layer(layer));
            }

            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.disable(glow::DEPTH_TEST);
        }
    }

    /// Compiles the shaders on the first frame and uploads waiting data.
    fn prepare(&mut self, gl: &glow::Context) -> Result<(), String> {
        if self.programs.is_none() {
            self.programs = Some(Programs {
                shaded: Program::new(gl, SHADED_VERTEX, SHADED_FRAGMENT)?,
                lines: Program::new(gl, LINES_VERTEX, LINES_FRAGMENT)?,
            });
        }
        if let Some((plate, grid)) = self.pending_bed.take() {
            replace(gl, &mut self.plate, Buffers::triangles(gl, &plate))?;
            replace(gl, &mut self.grid, Buffers::lines(gl, grid))?;
        }
        if let Some(models) = self.pending_models.take() {
            for old in self.models.drain(..) {
                old.delete(gl);
            }
            for model in &models {
                self.models.push(Buffers::triangles(gl, model)?);
            }
        }
        if let Some(paths) = self.pending_paths.take() {
            replace(gl, &mut self.paths, Buffers::lines(gl, paths))?;
        }
        Ok(())
    }
}
