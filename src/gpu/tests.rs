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

fn kind_of<T: std::fmt::Debug>(result: Result<T, OsError>) -> OsErrorKind {
    result.expect_err("expected an error").kind
}

fn write_cmd(buffer: &Value, offset: i64, data: Value) -> Value {
    record([(
        "write",
        record([
            ("buffer", buffer.clone()),
            ("offset", Value::Int(offset)),
            ("data", data),
        ]),
    )])
}

fn read_cmd(buffer: &Value, offset: i64, length: i64) -> Value {
    record([(
        "read",
        record([
            ("buffer", buffer.clone()),
            ("offset", Value::Int(offset)),
            ("length", Value::Int(length)),
        ]),
    )])
}

/// Submit `commands` as one job; the bytes of each read, in list order.
fn job(host: &GpuHost, device: &Value, commands: Vec<Value>) -> Result<Vec<Vec<u8>>, OsError> {
    let reply = call(
        host,
        OsOp::GpuSubmit,
        &[device.clone(), Value::array(commands)],
    )?;
    Ok(reply
        .get("reads")
        .and_then(|r| r.as_array())
        .expect("reads")
        .iter()
        .map(|r| r.as_bytes().expect("bytes").to_vec())
        .collect())
}

fn write(
    host: &GpuHost,
    device: &Value,
    buffer: &Value,
    offset: i64,
    data: Value,
) -> Result<(), OsError> {
    job(host, device, vec![write_cmd(buffer, offset, data)]).map(|_| ())
}

fn read(
    host: &GpuHost,
    device: &Value,
    buffer: &Value,
    offset: i64,
    length: i64,
) -> Result<Vec<u8>, OsError> {
    job(host, device, vec![read_cmd(buffer, offset, length)]).map(|mut reads| reads.remove(0))
}

fn u32s(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[test]
fn vector_add_runs_on_the_gpu_and_reads_back() {
    let Some((host, device)) = open() else { return };
    let usage = ["storage", "copy-dst", "copy-src"];
    let a = buffer(&host, &device, 32, &usage).unwrap();
    let b = buffer(&host, &device, 32, &usage).unwrap();
    let out = buffer(&host, &device, 32, &usage).unwrap();
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
    let reads = job(
        &host,
        &device,
        vec![
            write_cmd(&a, 0, u32_bytes(&[1, 2, 3, 4, 5, 6, 7, 8])),
            write_cmd(&b, 0, u32_bytes(&[10, 20, 30, 40, 50, 60, 70, 80])),
            read_cmd(&out, 0, 32),
            command,
            read_cmd(&out, 0, 32),
            read_cmd(&a, 0, 4),
        ],
    )
    .unwrap();
    assert_eq!(reads.len(), 3, "one bytes per read");
    assert_eq!(
        u32s(&reads[0]),
        vec![0; 8],
        "a read before the dispatch sees the buffer before it ran"
    );
    assert_eq!(u32s(&reads[1]), vec![11, 22, 33, 44, 55, 66, 77, 88]);
    assert_eq!(
        u32s(&reads[2]),
        vec![1],
        "control: the inputs were not touched"
    );
}

#[test]
fn a_texture_write_repacks_its_rows_and_lands_in_order() {
    let Some((host, device)) = open() else { return };
    let texture = handle(
        &call(
            &host,
            OsOp::GpuTextureCreate,
            &[
                device.clone(),
                record([
                    ("width", Value::Int(3)),
                    ("height", Value::Int(2)),
                    ("format", text("r32float")),
                    ("usage", names(&["texture-binding", "copy-dst"])),
                ]),
            ],
        )
        .unwrap(),
        "texture",
    );
    let out = buffer(&host, &device, 24, &["storage", "copy-src"]).unwrap();
    let wgsl = "
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<f32>;
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    out[id.x + id.y * 3u] = textureLoad(t, vec2<i32>(i32(id.x), i32(id.y)), 0).r;
}
";
    let shader = handle(
        &call(&host, OsOp::GpuShaderCreate, &[device.clone(), text(wgsl)]).unwrap(),
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
    let entries = Value::array(vec![
        record([("binding", Value::Int(0)), ("texture", texture.clone())]),
        record([("binding", Value::Int(1)), ("buffer", out.clone())]),
    ]);
    let group = handle(
        &call(
            &host,
            OsOp::GpuBindGroupCreate,
            &[pipeline.clone(), Value::Int(0), entries],
        )
        .unwrap(),
        "bindGroup",
    );
    let texels: Vec<u8> = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let compute = record([(
        "compute",
        record([
            ("pipeline", pipeline),
            ("bindGroups", Value::array(vec![group])),
            ("dispatch", ints(&[3, 2, 1])),
        ]),
    )]);
    let write = record([(
        "write",
        record([("texture", texture.clone()), ("data", Value::bytes(texels))]),
    )]);
    let reads = job(&host, &device, vec![write, compute, read_cmd(&out, 0, 24)]).unwrap();
    let got: Vec<f32> = reads[0]
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(
        got,
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        "a 12-byte row padded to 256 in the upload lands row by row"
    );
    let short = record([(
        "write",
        record([("texture", texture), ("data", Value::bytes(vec![0; 20]))]),
    )]);
    assert_eq!(
        kind_of(job(&host, &device, vec![short])),
        OsErrorKind::Invalid,
        "a texture write carries the whole texture"
    );
}

#[test]
fn a_map_read_buffer_reads_after_the_list_and_no_later_command_may_write_it() {
    let Some((host, device)) = open() else { return };
    let src = buffer(&host, &device, 16, &["copy-src", "copy-dst"]).unwrap();
    let mapped = buffer(&host, &device, 16, &["map-read", "copy-dst"]).unwrap();
    let copy = record([(
        "copy",
        record([("from", src.clone()), ("to", mapped.clone())]),
    )]);
    let reads = job(
        &host,
        &device,
        vec![
            write_cmd(&src, 0, u32_bytes(&[7, 8, 9, 10])),
            copy.clone(),
            read_cmd(&mapped, 0, 16),
            read_cmd(&mapped, 8, 8),
        ],
    )
    .unwrap();
    assert_eq!(u32s(&reads[0]), vec![7, 8, 9, 10]);
    assert_eq!(
        u32s(&reads[1]),
        vec![9, 10],
        "two reads of one map-read buffer map it once"
    );
    assert_eq!(
        kind_of(job(&host, &device, vec![read_cmd(&mapped, 0, 16), copy])),
        OsErrorKind::Invalid,
        "a later copy into it would make the read lie about its place in the list"
    );
    assert_eq!(
        kind_of(read(&host, &device, &mapped, 4, 8)),
        OsErrorKind::Invalid,
        "a map-read read starts at a multiple of 8"
    );
}

#[test]
fn a_write_or_read_off_a_multiple_of_four_or_past_the_buffer_is_invalid() {
    let Some((host, device)) = open() else { return };
    let buf = buffer(&host, &device, 16, &["copy-src", "copy-dst"]).unwrap();
    assert_eq!(
        kind_of(write(&host, &device, &buf, 0, Value::bytes(vec![1, 2, 3]))),
        OsErrorKind::Invalid
    );
    assert_eq!(
        kind_of(write(
            &host,
            &device,
            &buf,
            2,
            Value::bytes(vec![1, 2, 3, 4])
        )),
        OsErrorKind::Invalid
    );
    assert_eq!(
        kind_of(write(
            &host,
            &device,
            &buf,
            16,
            Value::bytes(vec![1, 2, 3, 4])
        )),
        OsErrorKind::Invalid
    );
    assert_eq!(
        kind_of(read(&host, &device, &buf, 0, 6)),
        OsErrorKind::Invalid
    );
    assert_eq!(
        kind_of(read(&host, &device, &buf, 12, 8)),
        OsErrorKind::Invalid
    );
    let storage_only = buffer(&host, &device, 16, &["storage"]).unwrap();
    assert_eq!(
        kind_of(read(&host, &device, &storage_only, 0, 16)),
        OsErrorKind::Invalid,
        "a read needs copy-src or map-read"
    );
    write(&host, &device, &buf, 4, u32_bytes(&[5])).unwrap();
    assert_eq!(
        u32s(&read(&host, &device, &buf, 0, 8).unwrap()),
        vec![0, 5],
        "control"
    );
    let empty = call(
        &host,
        OsOp::GpuSubmit,
        &[
            device.clone(),
            Value::array(vec![write_cmd(&buf, 0, u32_bytes(&[1]))]),
        ],
    )
    .unwrap();
    assert_eq!(
        empty.get("reads").and_then(|r| r.as_array()).map(Vec::len),
        Some(0),
        "a job without a read answers an empty reads list"
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
        &[device.clone(), Value::array(vec![render, copy])],
    )
    .unwrap();
    read(&host, &device, &readback, 0, 512).ok()
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
    assert_eq!(
        kind_of(read(&host, &device, &dropped, 0, 16)),
        OsErrorKind::Closed
    );
    assert_eq!(
        kind_of(call(&host, OsOp::GpuRelease, &[dropped])),
        OsErrorKind::Closed
    );
    assert!(read(&host, &device, &kept, 0, 16).is_ok());
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
    assert_eq!(
        kind_of(read(&host, &device, &owned, 0, 16)),
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

#[test]
fn reads_reuse_one_staging_buffer_and_return_only_their_own_bytes() {
    let Some((host, device)) = open() else { return };
    let buf = buffer(&host, &device, 32, &["copy-dst", "copy-src"]).unwrap();
    write(
        &host,
        &device,
        &buf,
        0,
        u32_bytes(&[1, 2, 3, 4, 5, 6, 7, 8]),
    )
    .unwrap();
    let read = |offset: i64, length: i64| read(&host, &device, &buf, offset, length).unwrap();
    let kept = |host: &GpuHost| host.staging.lock().unwrap().as_ref().map(|b| b.size());
    assert_eq!(kept(&host), None, "control: no read yet, nothing kept");
    assert_eq!(
        read(0, 32),
        u32_bytes(&[1, 2, 3, 4, 5, 6, 7, 8]).as_bytes().unwrap()
    );
    assert_eq!(kept(&host), Some(32));
    let first = host.staging.lock().unwrap().clone().unwrap();
    assert_eq!(
        read(8, 8),
        u32_bytes(&[3, 4]).as_bytes().unwrap(),
        "a shorter read through the kept buffer returns only its bytes"
    );
    assert!(
        host.staging.lock().unwrap().as_ref() == Some(&first),
        "the kept buffer was reused"
    );
    write(
        &host,
        &device,
        &buf,
        0,
        u32_bytes(&[9, 9, 9, 9, 9, 9, 9, 9]),
    )
    .unwrap();
    assert_eq!(
        read(0, 32),
        u32_bytes(&[9; 8]).as_bytes().unwrap(),
        "reuse never returns stale bytes"
    );
    call(&host, OsOp::GpuDeviceClose, &[device]).unwrap();
    assert_eq!(
        kept(&host),
        None,
        "closing the device drops the kept buffer"
    );
}

#[test]
fn a_live_buffer_resolves_for_drawing_and_control_released_or_on_another_device_it_does_not() {
    let Some((host, device)) = open() else { return };
    let made = buffer(&host, &device, 64, &["storage"]).expect("buffer");
    let id = made.as_int().expect("buffer id") as u64;
    let dev = device.as_int().expect("device id") as u64;
    assert!(matches!(
        super::drawable::resolve(id, dev),
        Ok(super::drawable::DrawableGpu::Buffer { size: 64, .. })
    ));
    assert_eq!(
        super::drawable::resolve(id, dev + 1_000_000).err(),
        Some(super::drawable::DrawableGpuError::WrongDevice {
            id,
            device: dev + 1_000_000
        })
    );
    call(&host, OsOp::GpuRelease, &[made.clone()]).expect("release");
    assert_eq!(
        super::drawable::resolve(id, dev).err(),
        Some(super::drawable::DrawableGpuError::NotLive { id })
    );
}
