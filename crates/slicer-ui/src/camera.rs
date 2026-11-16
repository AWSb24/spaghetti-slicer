//! Orbit camera of the 3D view: it turns around a point, pans and zooms,
//! and gives the matrices to draw with.
//!
//! The world is the bed in millimetres with Z up. Matrices are 4×4,
//! column-major, as OpenGL expects them. The angles use std `sin`/`cos`;
//! this is display only and never reaches the G-code.

/// A 4×4 matrix, column-major.
pub type Mat4 = [f32; 16];

/// Vertical field of view in degrees.
const FOV_Y_DEG: f32 = 35.0;
/// Degrees the camera turns per pixel of mouse movement.
const DEG_PER_PIXEL: f32 = 0.4;
/// The camera stays this far from straight above or below, so that "up"
/// stays defined.
const PITCH_LIMIT_DEG: f32 = 89.0;
/// Closest and farthest distance from the target in millimetres.
pub const DISTANCE_RANGE: (f32, f32) = (1.0, 20_000.0);

/// A camera on a sphere around a target point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    /// The point looked at, in millimetres.
    pub target: [f32; 3],
    /// Direction from the target to the camera around Z, in degrees; 0
    /// is +X, -90 is in front of the bed (-Y).
    pub yaw_deg: f32,
    /// Height of the camera above the bed plane as an angle, in degrees.
    pub pitch_deg: f32,
    /// Distance from the target in millimetres.
    pub distance: f32,
}

impl OrbitCamera {
    /// Looks at the middle of a bed of `bed` millimetres from the front
    /// right and a little above, far enough to see the whole build volume
    /// up to `height`.
    pub fn fit(bed: [f64; 2], height: f64) -> Self {
        let size = [bed[0] as f32, bed[1] as f32, height as f32];
        let radius = 0.5 * dot(size, size).sqrt();
        let half_fov = (FOV_Y_DEG / 2.0).to_radians();
        let (min, max) = DISTANCE_RANGE;
        Self {
            target: [size[0] / 2.0, size[1] / 2.0, size[2] / 4.0],
            yaw_deg: -60.0,
            pitch_deg: 30.0,
            // A little margin: the target sits below the volume's middle.
            distance: (1.15 * radius / half_fov.sin()).clamp(min, max),
        }
    }

    /// Unit vector from the target to the camera.
    fn backward(&self) -> [f32; 3] {
        let (sin_yaw, cos_yaw) = self.yaw_deg.to_radians().sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch_deg.to_radians().sin_cos();
        [cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch]
    }

    /// Position of the camera.
    pub fn eye(&self) -> [f32; 3] {
        add(self.target, scale(self.backward(), self.distance))
    }

    /// Unit vectors pointing right and up on the screen, in the world.
    fn right_up(&self) -> ([f32; 3], [f32; 3]) {
        let forward = scale(self.backward(), -1.0);
        let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
        (right, cross(right, forward))
    }

    /// Direction towards the camera, for a light that comes from the
    /// viewer and a little from above.
    pub fn light_direction(&self) -> [f32; 3] {
        let (_, up) = self.right_up();
        normalize(add(self.backward(), scale(up, 0.5)))
    }

    /// Turns the camera around the target after a mouse movement of
    /// `dx`, `dy` pixels: sideways around Z, up and down over the bed.
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw_deg = (self.yaw_deg - dx * DEG_PER_PIXEL) % 360.0;
        self.pitch_deg =
            (self.pitch_deg + dy * DEG_PER_PIXEL).clamp(-PITCH_LIMIT_DEG, PITCH_LIMIT_DEG);
    }

    /// Moves the target so that the scene follows a mouse movement of
    /// `dx`, `dy` pixels in a view `viewport_height` pixels high: the
    /// target stays under the cursor.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let per_pixel =
            2.0 * self.distance * (FOV_Y_DEG / 2.0).to_radians().tan() / viewport_height.max(1.0);
        let (right, up) = self.right_up();
        let shift = add(scale(right, -dx * per_pixel), scale(up, dy * per_pixel));
        self.target = add(self.target, shift);
    }

    /// Moves closer to the target by `factor` (above 1) or away from it.
    pub fn zoom(&mut self, factor: f32) {
        let (min, max) = DISTANCE_RANGE;
        if factor > 0.0 {
            self.distance = (self.distance / factor).clamp(min, max);
        }
    }

    /// World to camera coordinates.
    pub fn view(&self) -> Mat4 {
        let eye = self.eye();
        let forward = scale(self.backward(), -1.0);
        let (right, up) = self.right_up();
        [
            right[0],
            up[0],
            -forward[0],
            0.0,
            right[1],
            up[1],
            -forward[1],
            0.0,
            right[2],
            up[2],
            -forward[2],
            0.0,
            -dot(right, eye),
            -dot(up, eye),
            dot(forward, eye),
            1.0,
        ]
    }

    /// Camera to clip coordinates for a view `aspect` times as wide as it
    /// is high.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let near = self.distance / 100.0;
        let far = self.distance * 100.0;
        let f = 1.0 / (FOV_Y_DEG / 2.0).to_radians().tan();
        let mut m = [0.0; 16];
        m[0] = f / aspect.max(1e-6);
        m[5] = f;
        m[10] = (far + near) / (near - far);
        m[11] = -1.0;
        m[14] = 2.0 * far * near / (near - far);
        m
    }

    /// World to clip coordinates.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        multiply(&self.projection(aspect), &self.view())
    }
}

/// `a * b`.
pub fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut m = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            m[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    m
}

/// A point mapped by `m` and divided by w: normalized device coordinates
/// for a view-projection matrix, from -1 to 1 on screen. `None` behind
/// the camera.
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> Option<[f32; 3]> {
    let row = |r: usize| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
    let w = row(3);
    (w > 0.0).then(|| [row(0) / w, row(1) / w, row(2) / w])
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let length = dot(a, a).sqrt();
    if length > 0.0 {
        scale(a, 1.0 / length)
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const BED: [f64; 2] = [256.0, 256.0];
    const ASPECT: f32 = 1.5;

    fn ndc(camera: &OrbitCamera, p: [f32; 3]) -> [f32; 3] {
        transform_point(&camera.view_projection(ASPECT), p).unwrap()
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn target_is_in_the_middle_and_the_bed_fits() {
        let camera = OrbitCamera::fit(BED, 250.0);
        let centre = ndc(&camera, camera.target);
        assert!(near(centre[0], 0.0) && near(centre[1], 0.0));
        let eye = camera.eye();
        let offset = [
            eye[0] - camera.target[0],
            eye[1] - camera.target[1],
            eye[2] - camera.target[2],
        ];
        assert!((dot(offset, offset).sqrt() - camera.distance).abs() < 1e-2);
        // In front of the bed (lower Y) and above it.
        assert!(eye[1] < 0.0 && eye[2] > 0.0);
        for corner in [
            [0.0, 0.0, 0.0],
            [256.0, 0.0, 0.0],
            [0.0, 256.0, 250.0],
            [256.0, 256.0, 250.0],
        ] {
            let p = ndc(&camera, corner);
            assert!(p.iter().all(|c| c.abs() < 1.0), "{corner:?} -> {p:?}");
        }
    }

    #[test]
    fn up_is_up_and_right_is_right() {
        let camera = OrbitCamera::fit(BED, 250.0);
        let t = camera.target;
        assert!(ndc(&camera, [t[0], t[1], t[2] + 10.0])[1] > 0.0);
        let (right, _) = camera.right_up();
        assert!(near(right[2], 0.0));
        assert!(ndc(&camera, add(t, scale(right, 10.0)))[0] > 0.0);
        // Seen from the front, +X is to the right.
        let front = OrbitCamera {
            yaw_deg: -90.0,
            pitch_deg: 0.0,
            ..camera
        };
        assert!(ndc(&front, [t[0] + 10.0, t[1], t[2]])[0] > 0.0);
    }

    #[test]
    fn nearer_points_have_smaller_depth() {
        let camera = OrbitCamera::fit(BED, 250.0);
        let toward_eye = add(camera.target, scale(camera.backward(), 10.0));
        assert!(ndc(&camera, toward_eye)[2] < ndc(&camera, camera.target)[2]);
        // Behind the camera there is nothing to draw.
        let behind = add(camera.eye(), scale(camera.backward(), 1.0));
        assert!(transform_point(&camera.view_projection(ASPECT), behind).is_none());
    }

    #[test]
    fn pan_keeps_the_target_under_the_cursor() {
        let mut camera = OrbitCamera::fit(BED, 250.0);
        let before = camera.target;
        let (width, height) = (900.0, 600.0);
        camera.pan(30.0, -12.0, height);
        // The old target moved with the cursor: 30 px right, 12 px up.
        let p = ndc(&camera, before);
        assert!(near(p[0], 2.0 * 30.0 / width), "{p:?}");
        assert!(near(p[1], 2.0 * 12.0 / height), "{p:?}");
    }

    #[test]
    fn rotation_and_zoom_stay_in_range() {
        let mut camera = OrbitCamera::fit(BED, 250.0);
        let distance = camera.distance;
        camera.zoom(2.0);
        assert!(near(camera.distance, distance / 2.0));
        camera.zoom(1e9);
        assert_eq!(camera.distance, DISTANCE_RANGE.0);
        camera.zoom(0.0);
        assert_eq!(camera.distance, DISTANCE_RANGE.0);
        camera.rotate(0.0, 1000.0);
        assert_eq!(camera.pitch_deg, PITCH_LIMIT_DEG);
        let yaw = camera.yaw_deg;
        camera.rotate(10.0, 0.0);
        assert!(near(camera.yaw_deg, yaw - 4.0));
        // Straight from above the view is still defined.
        assert!(ndc(&camera, camera.target).iter().all(|c| c.is_finite()));
    }

    #[test]
    fn light_comes_from_the_viewer() {
        let camera = OrbitCamera::fit(BED, 250.0);
        let light = camera.light_direction();
        assert!(near(dot(light, light), 1.0));
        assert!(dot(light, camera.backward()) > 0.5);
        assert!(light[2] > camera.backward()[2]);
    }

    #[test]
    fn matrix_product() {
        let mut shift = [0.0; 16];
        for i in 0..4 {
            shift[i * 5] = 1.0;
        }
        shift[12] = 5.0;
        let mut doubled = [0.0; 16];
        for i in 0..3 {
            doubled[i * 5] = 2.0;
        }
        doubled[15] = 1.0;
        // Double first, then shift.
        let both = multiply(&shift, &doubled);
        assert_eq!(
            transform_point(&both, [1.0, 2.0, 3.0]),
            Some([7.0, 4.0, 6.0])
        );
    }
}
