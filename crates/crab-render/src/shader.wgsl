// Textured block shader: sample the atlas, multiply by per-face tint, and apply
// simple directional + ambient lighting so 3D structure reads clearly.

struct Camera {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) normal: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.tint = in.tint;
    out.normal = in.normal;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas_tex, atlas_samp, in.uv);
    if sampled.a < 0.5 {
        discard;
    }
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.25));
    let n = normalize(in.normal);
    let diffuse = max(dot(n, light_dir), 0.0);
    let shade = 0.45 + 0.55 * diffuse;
    return vec4<f32>(sampled.rgb * in.tint * shade, 1.0);
}
