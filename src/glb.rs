//! 메시를 glTF 2.0 바이너리(.glb)로 쓴다. 메시 1개, 회색 재질 1개, 색 속성 없음.

use serde_json::json;

use crate::mesh::Mesh;

/// 재질의 기본 회색(선형).
const GRAY: f32 = 0.6;

fn pad(buf: &mut Vec<u8>, byte: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(byte);
    }
}

/// 정점은 모두 공유한다(색 경계가 없다). 메시에 정점 법선이 있으면 NORMAL 을 쓴다.
/// glTF 뷰어는 NORMAL 이 없으면 flat normal 을 계산하고, Blender 는 면을 flat 으로 가져온다.
pub fn encode(mesh: &Mesh) -> Vec<u8> {
    let indices: Vec<u32> = mesh.tris.iter().flatten().copied().collect();
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut bin = Vec::new();
    for v in &mesh.positions {
        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let pos_len = bin.len();
    let smooth = !mesh.positions.is_empty() && mesh.normals.len() == mesh.positions.len();
    if smooth {
        for n in &mesh.normals {
            for c in [n.x, n.y, n.z] {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
    let nrm_len = bin.len() - pos_len;
    let wide = mesh.positions.len() > u16::MAX as usize;
    for &i in &indices {
        if wide {
            bin.extend_from_slice(&i.to_le_bytes());
        } else {
            bin.extend_from_slice(&(i as u16).to_le_bytes());
        }
    }
    let idx_len = bin.len() - pos_len - nrm_len;
    pad(&mut bin, 0);

    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "mytripo_public"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "mytripo_public"}],
        "meshes": [{"name": "mytripo_public", "primitives": [{
            "attributes": {"POSITION": 0},
            "indices": 1,
            "material": 0,
            "mode": 4
        }]}],
        "materials": [{
            "name": "mytripo_public",
            "pbrMetallicRoughness": {"baseColorFactor": [GRAY, GRAY, GRAY, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.9}
        }],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": pos_len, "target": 34962},
            {"buffer": 0, "byteOffset": pos_len, "byteLength": idx_len, "target": 34963}
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": mesh.positions.len(), "type": "VEC3", "min": lo, "max": hi},
            {"bufferView": 1, "componentType": if wide { 5125 } else { 5123 }, "count": indices.len(), "type": "SCALAR"}
        ]
    });
    if smooth {
        // 법선은 위치와 번호 사이에 있다. bufferView 2, accessor 2 로 더한다.
        doc["bufferViews"][1]["byteOffset"] = json!(pos_len + nrm_len);
        doc["bufferViews"]
            .as_array_mut()
            .expect("bufferViews")
            .push(
                json!({"buffer": 0, "byteOffset": pos_len, "byteLength": nrm_len, "target": 34962}),
            );
        doc["accessors"]
            .as_array_mut()
            .expect("accessors")
            .push(json!({
                "bufferView": 2, "componentType": 5126, "count": mesh.normals.len(), "type": "VEC3"
            }));
        doc["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(2);
    }
    let mut js = serde_json::to_vec(&doc).expect("glTF JSON");
    pad(&mut js, b' ');

    let total = 12 + 8 + js.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&js);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::V3;

    #[test]
    fn glb_has_one_mesh_one_gray_material_and_no_colors() {
        let mesh = Mesh {
            positions: vec![
                V3::new(0.0, 0.0, 0.0),
                V3::new(1.0, 0.0, 0.0),
                V3::new(0.0, 1.0, 0.0),
                V3::new(1.0, 1.0, 0.0),
            ],
            tris: vec![[0, 1, 2], [1, 3, 2]],
            parts: vec![0, 0],
            normals: Vec::new(),
        };
        let b = encode(&mesh);
        assert_eq!(&b[0..4], b"glTF");
        assert_eq!(
            u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize,
            b.len()
        );
        let jl = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        assert_eq!(&b[16..20], b"JSON");
        let doc: serde_json::Value = serde_json::from_slice(&b[20..20 + jl]).unwrap();
        assert_eq!(doc["meshes"].as_array().unwrap().len(), 1);
        assert_eq!(doc["materials"].as_array().unwrap().len(), 1);
        assert!(
            doc["meshes"][0]["primitives"][0]["attributes"]
                .get("COLOR_0")
                .is_none()
        );
        assert_eq!(doc["accessors"][0]["count"], 4);
        assert_eq!(doc["accessors"][1]["count"], 6);
        assert_eq!(&b[24 + jl..28 + jl], b"BIN\0");
        assert_eq!(b.len() % 4, 0);
        // 정점 법선이 없으면 NORMAL 을 쓰지 않는다.
        assert!(
            doc["meshes"][0]["primitives"][0]["attributes"]
                .get("NORMAL")
                .is_none()
        );

        // 정점 법선이 있으면 NORMAL 을 쓴다. 번호 bufferView 는 법선 뒤로 밀린다.
        let mut smooth = mesh.clone();
        smooth.normals = smooth.smooth_normals();
        let b = encode(&smooth);
        let jl = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        let doc: serde_json::Value = serde_json::from_slice(&b[20..20 + jl]).unwrap();
        assert_eq!(doc["meshes"][0]["primitives"][0]["attributes"]["NORMAL"], 2);
        assert_eq!(doc["accessors"][2]["count"], 4);
        assert_eq!(doc["bufferViews"][2]["byteOffset"], 48);
        assert_eq!(doc["bufferViews"][1]["byteOffset"], 96);
        assert_eq!(
            u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize,
            b.len()
        );
    }
}
