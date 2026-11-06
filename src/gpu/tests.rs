//! End-to-end through `GpuHost::answer`. Every test returns early when the machine has no adapter.

use super::{record, GpuHost};
use orbital_core_base::gpu::GpuAnswerer;
use orbital_core_base::os::{OsError, OsErrorKind, OsOp};
use orbital_core_base::value::Value;

fn names(items: &[&str]) -> Value {
    Value::array(
        items
            .iter()
            .map(|s| Value::string(s.to_string()))
            .collect::<Vec<_>>(),
    )
}

fn ints(items: &[i64]) -> Value {
    Value::array(items.iter().map(|n| Value::Int(*n)).collect::<Vec<_>>())
}

fn text(s: &str) -> Value {
    Value::string(s.to_string())
}

fn call(host: &GpuHost, op: OsOp, args: &[Value]) -> Result<Value, OsError> {
    host.answer(op, args)
}

fn handle(reply: &Value, key: &str) -> Value {
    reply.get(key).cloned().expect("reply field")
}

fn open() -> Option<(GpuHost, Value)> {
    let host = GpuHost::new();
    match call(&host, OsOp::GpuDeviceOpen, &[record([])]) {
        Ok(reply) => {
            let device = handle(&reply, "device");
            Some((host, device))
        }
        Err(_) => None,
    }
}

fn buffer(host: &GpuHost, device: &Value, size: i64, usage: &[&str]) -> Result<Value, OsError> {
    let reply = call(
        host,
        OsOp::GpuBufferCreate,
        &[
            device.clone(),
            record([("size", Value::Int(size)), ("usage", names(usage))]),
        ],
    )?;
    Ok(handle(&reply, "buffer"))
}

fn u32_bytes(items: &[u32]) -> Value {
    Value::bytes(
        items
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect::<Vec<u8>>(),
    )
}

const ADD: &str = "
@group(0) @binding(0) var<storage, read> a: array<u32>;
@group(0) @binding(1) var<storage, read> b: array<u32>;
@group(0) @binding(2) var<storage, read_write> out: array<u32>;
@compute @workgroup_size(4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    out[id.x] = a[id.x] + b[id.x];
}
";

const TRIANGLE: &str = "
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}
@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
";

fn kind_of(result: Result<Value, OsError>) -> OsErrorKind {
    result.expect_err("expected an error").kind
}

#[test]
fn vector_add_runs_on_the_gpu_and_reads_back() {
    let Some((host, device)) = open() else { return };
    let usage = ["storage", "copy-dst", "copy-src"];
    let a = buffer(&host, &device, 32, &usage).unwrap();
    let b = buffer(&host, &device, 32, &usage).unwrap();
    let out = buffer(&host, &device, 32, &usage).unwrap();
    call(
        &host,
        OsOp::GpuWrite,
        &[
            a.clone(),
            Value::Int(0),
            u32_bytes(&[1, 2, 3, 4, 5, 6, 7, 8]),
        ],
    )
    .unwrap();
    call(
        &host,
        OsOp::GpuWrite,
        &[
            b.clone(),
            Value::Int(0),
            u32_bytes(&[10, 20, 30, 40, 50, 60, 70, 80]),
        ],
    )
    .unwrap();
    let shader = handle(
        &call(&host, OsOp::GpuShaderCreate, &[device.clone(), text(ADD)]).unwrap(),
        "shader",
    );
    let pipeline = handle(
        &call(
            &host,
            OsOp::GpuPipelineCreate,
            &[
                device.clone(),
                record([
                    ("kind", text("compute")),
                    ("shader", shader),
                    ("entry", text("main")),
                ]),
            ],
        )
        .unwrap(),
        "pipeline",
    );
    let entries = Value::array(
        [(0, &a), (1, &b), (2, &out)]
            .iter()
            .map(|(binding, buf)| {
                record([
                    ("binding", Value::Int(*binding)),
                    ("buffer", (*buf).clone()),
                ])
            })
            .collect::<Vec<_>>(),
    );
    let group = handle(
        &call(
            &host,
            OsOp::GpuBindGroupCreate,
            &[pipeline.clone(), Value::Int(0), entries],
        )
        .unwrap(),
        "bindGroup",
    );
    let command = record([(
        "compute",
        record([
            ("pipeline", pipeline),
            ("bindGroups", Value::array(vec![group])),
            ("dispatch", ints(&[2, 1, 1])),
        ]),
    )]);
    call(
        &host,
        OsOp::GpuSubmit,
        &[device.clone(), Value::array(vec![command])],
    )
    .unwrap();
    let read = call(&host, OsOp::GpuRead, &[out, Value::Int(0), Value::Int(32)]).unwrap();
    let bytes = read
        .get("data")
        .and_then(|d| d.as_bytes())
        .expect("bytes")
        .to_vec();
    let sums: Vec<u32> = bytes
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(sums, vec![11, 22, 33, 44, 55, 66, 77, 88]);
    // Control: the inputs were not touched.
    let first = call(&host, OsOp::GpuRead, &[a, Value::Int(0), Value::Int(4)]).unwrap();
    assert_eq!(
        first.get("data").and_then(|d| d.as_bytes()),
        Some(&1u32.to_le_bytes()[..])
    );
}

fn render_to_pixels(draw_vertices: i64) -> Option<Vec<u8>> {
    let (host, device) = open()?;
    let texture = handle(
        &call(
            &host,
            OsOp::GpuTextureCreate,
            &[
                device.clone(),
                record([
                    ("width", Value::Int(64)),
                    ("height", Value::Int(2)),
                    ("format", text("rgba8unorm")),
                    ("usage", names(&["render-attachment", "copy-src"])),
                ]),
            ],
        )
        .unwrap(),
        "texture",
    );
    let readback = buffer(&host, &device, 512, &["copy-dst", "copy-src"]).unwrap();
    let shader = handle(
        &call(
            &host,
            OsOp::GpuShaderCreate,
            &[device.clone(), text(TRIANGLE)],
        )
        .unwrap(),
        "shader",
    );
    let pipeline = handle(
        &call(
            &host,
            OsOp::GpuPipelineCreate,
            &[
                device.clone(),
                record([
                    ("kind", text("render")),
                    ("shader", shader),
                    ("entry", text("vs")),
                    ("fragment", text("fs")),
                    ("targets", names(&["rgba8unorm"])),
                ]),
            ],
        )
        .unwrap(),
        "pipeline",
    );
    let render = record([(
        "render",
        record([
            ("pipeline", pipeline),
            ("bindGroups", Value::array(Vec::<Value>::new())),
            ("target", texture.clone()),
            ("vertices", Value::Int(draw_vertices)),
            (
                "clear",
                Value::array(vec![
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(1.0),
                    Value::Number(1.0),
                ]),
            ),
        ]),
    )]);
    let copy = record([(
        "copy",
        record([("from", texture), ("to", readback.clone())]),
    )]);
    call(
        &host,
        OsOp::GpuSubmit,
        &[device, Value::array(vec![render, copy])],
    )
    .unwrap();
    let read = call(
        &host,
        OsOp::GpuRead,
        &[readback, Value::Int(0), Value::Int(512)],
    )
    .unwrap();
    read.get("data")
        .and_then(|d| d.as_bytes())
        .map(<[u8]>::to_vec)
}

#[test]
fn an_offscreen_render_draws_into_a_texture_that_reads_back() {
    let Some(pixels) = render_to_pixels(3) else {
        return;
    };
    assert_eq!(pixels.len(), 512);
    assert!(pixels.chunks(4).all(|p| p == [255, 0, 0, 255]));
}

#[test]
fn a_render_with_no_vertices_only_clears() {
    let Some(pixels) = render_to_pixels(0) else {
        return;
    };
    assert!(pixels.chunks(4).all(|p| p == [0, 0, 255, 255]));
}

#[test]
fn invalid_wgsl_answers_invalid_with_the_message() {
    let Some((host, device)) = open() else { return };
    let error = call(
        &host,
        OsOp::GpuShaderCreate,
        &[device.clone(), text("fn main( {")],
    )
    .unwrap_err();
    assert_eq!(error.kind, OsErrorKind::Invalid);
    assert!(!error.message.is_empty());
    // Control: the same device still takes valid WGSL afterwards.
    assert!(call(&host, OsOp::GpuShaderCreate, &[device, text(ADD)]).is_ok());
}

#[test]
fn an_unknown_usage_answers_invalid() {
    let Some((host, device)) = open() else { return };
    assert_eq!(
        kind_of(buffer(&host, &device, 16, &["storage", "bogus"])),
        OsErrorKind::Invalid
    );
    assert_eq!(
        kind_of(buffer(&host, &device, 16, &[])),
        OsErrorKind::Invalid
    );
    assert!(buffer(&host, &device, 16, &["storage"]).is_ok());
}

#[test]
fn a_released_buffer_answers_closed() {
    let Some((host, device)) = open() else { return };
    let kept = buffer(&host, &device, 16, &["copy-src", "copy-dst"]).unwrap();
    let dropped = buffer(&host, &device, 16, &["copy-src", "copy-dst"]).unwrap();
    call(&host, OsOp::GpuRelease, &[dropped.clone()]).unwrap();
    let read = [dropped.clone(), Value::Int(0), Value::Int(16)];
    assert_eq!(
        kind_of(call(&host, OsOp::GpuRead, &read)),
        OsErrorKind::Closed
    );
    assert_eq!(
        kind_of(call(&host, OsOp::GpuRelease, &[dropped])),
        OsErrorKind::Closed
    );
    assert!(call(&host, OsOp::GpuRead, &[kept, Value::Int(0), Value::Int(16)]).is_ok());
}

#[test]
fn a_closed_device_answers_closed_for_itself_and_its_objects() {
    let Some((host, device)) = open() else { return };
    let owned = buffer(&host, &device, 16, &["copy-src"]).unwrap();
    let (other_host, other_device) = open().expect("second open");
    call(&host, OsOp::GpuDeviceClose, &[device.clone()]).unwrap();
    assert_eq!(
        kind_of(buffer(&host, &device, 16, &["storage"])),
        OsErrorKind::Closed
    );
    let read = [owned, Value::Int(0), Value::Int(16)];
    assert_eq!(
        kind_of(call(&host, OsOp::GpuRead, &read)),
        OsErrorKind::Closed
    );
    // Control: another host's device is unaffected.
    assert!(buffer(&other_host, &other_device, 16, &["storage"]).is_ok());
}

#[test]
fn handle_ids_are_never_reused() {
    let Some((host, device)) = open() else { return };
    let first = buffer(&host, &device, 16, &["storage"]).unwrap();
    call(&host, OsOp::GpuRelease, &[first.clone()]).unwrap();
    let second = buffer(&host, &device, 16, &["storage"]).unwrap();
    assert_ne!(first.as_int(), second.as_int());
}

#[test]
fn device_open_reports_compute_limits() {
    let host = GpuHost::new();
    let Ok(reply) = call(&host, OsOp::GpuDeviceOpen, &[record([])]) else {
        return;
    };
    let limits = reply.get("limits").expect("limits");
    assert!(
        limits
            .get("maxComputeWorkgroupsPerDimension")
            .and_then(|v| v.as_int())
            .unwrap_or(0)
            > 0
    );
    assert!(
        limits
            .get("maxStorageBuffersPerShaderStage")
            .and_then(|v| v.as_int())
            .unwrap_or(0)
            > 0
    );
    let bad = call(
        &host,
        OsOp::GpuDeviceOpen,
        &[record([("features", names(&["warp-drive"]))])],
    );
    assert_eq!(kind_of(bad), OsErrorKind::Invalid);
}
