// Textured block shader: sample the atlas, multiply by per-face tint, and apply
// simple directional + ambient lighting so 3D structure reads clearly.

struct Camera {
    view_proj: mat4x4<f32>,
    lighting: vec4<f32>,
    eye: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec3<f32>,
    @location(4) opacity: f32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) opacity: f32,
    @location(4) world_position: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.tint = in.tint;
    out.normal = in.normal;
    out.opacity = in.opacity;
    out.world_position = in.position;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas_tex, atlas_samp, in.uv);
    let alpha = sampled.a * in.opacity;
    if alpha < 0.02 {
        discard;
    }
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.25));
    let n = normalize(in.normal);
    let diffuse = max(dot(n, light_dir), 0.0);
    let shade = max(0.08, (0.35 + 0.65 * diffuse) * camera.lighting.x);
    let lit = sampled.rgb * in.tint * shade;
    let distance_from_eye = distance(in.world_position, camera.eye.xyz);
    let fog_amount = clamp(
        (distance_from_eye - camera.fog_params.x) /
            max(camera.fog_params.y - camera.fog_params.x, 0.01),
        0.0,
        1.0,
    ) * camera.fog_params.z;
    return vec4<f32>(mix(lit, camera.fog_color.rgb, fog_amount), alpha);
}
