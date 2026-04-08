#[cfg(windows)]
use anyhow::{anyhow, Result};
#[cfg(windows)]
use image::codecs::jpeg::JpegEncoder;
#[cfg(windows)]
use image::{DynamicImage, ImageBuffer, Rgba};
#[cfg(windows)]
use std::collections::{HashMap, HashSet};
#[cfg(windows)]
use std::io::Cursor;

#[cfg(windows)]
const DIRTY_TILE_SIZE: u32 = 64;

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
pub struct MoveRect {
    pub src_x: i32,
    pub src_y: i32,
    pub dst_x: i32,
    pub dst_y: i32,
    pub width: u32,
    pub height: u32,
}

#[cfg(windows)]
#[derive(Clone, Debug, Default)]
pub struct DeltaPlan {
    pub moves: Vec<MoveRect>,
    pub patches: Vec<DirtyRect>,
}

#[cfg(windows)]
struct HashedTile {
    rect: DirtyRect,
    hash: u64,
}

#[cfg(windows)]
pub fn detect_delta_plan(previous: &[u8], current: &[u8], width: u32, height: u32) -> Option<DeltaPlan> {
    if previous.len() != current.len() || previous.len() != (width * height * 4) as usize {
        return Some(DeltaPlan {
            moves: Vec::new(),
            patches: vec![DirtyRect {
                x: 0,
                y: 0,
                width,
                height,
            }],
        });
    }

    let mut changed_tiles = Vec::new();
    let mut previous_tiles: HashMap<u64, Vec<DirtyRect>> = HashMap::new();

    let mut tile_y = 0;
    while tile_y < height {
        let tile_height = DIRTY_TILE_SIZE.min(height - tile_y);
        let mut tile_x = 0;
        while tile_x < width {
            let tile_width = DIRTY_TILE_SIZE.min(width - tile_x);
            let rect = DirtyRect {
                x: tile_x,
                y: tile_y,
                width: tile_width,
                height: tile_height,
            };
            let previous_hash = hash_tile(previous, width, rect);
            previous_tiles.entry(previous_hash).or_default().push(rect);
            if tile_changed(previous, current, width, tile_x, tile_y, tile_width, tile_height) {
                changed_tiles.push(HashedTile {
                    rect,
                    hash: hash_tile(current, width, rect),
                });
            }
            tile_x += DIRTY_TILE_SIZE;
        }
        tile_y += DIRTY_TILE_SIZE;
    }

    if changed_tiles.is_empty() {
        return None;
    }

    let mut used_sources = HashSet::<DirtyRect>::new();
    let mut moves = Vec::new();
    let mut patches = Vec::new();

    for tile in changed_tiles {
        let moved_from = previous_tiles
            .get(&tile.hash)
            .and_then(|candidates| {
                candidates.iter().copied().find(|candidate| {
                    *candidate != tile.rect
                        && !used_sources.contains(candidate)
                        && tile_changed(
                            previous,
                            current,
                            width,
                            candidate.x,
                            candidate.y,
                            candidate.width,
                            candidate.height,
                        )
                })
            });

        if let Some(source) = moved_from {
            used_sources.insert(source);
            moves.push(MoveRect {
                src_x: source.x as i32,
                src_y: source.y as i32,
                dst_x: tile.rect.x as i32,
                dst_y: tile.rect.y as i32,
                width: tile.rect.width,
                height: tile.rect.height,
            });
        } else {
            patches.push(tile.rect);
        }
    }

    Some(DeltaPlan {
        moves: merge_move_rects(moves),
        patches: merge_dirty_rects(patches),
    })
}

#[cfg(windows)]
pub fn changed_ratio(plan: &DeltaPlan, width: u32, height: u32) -> f32 {
    let total = (width * height).max(1) as f32;
    let move_area: u64 = plan
        .moves
        .iter()
        .map(|rect| rect.width as u64 * rect.height as u64)
        .sum();
    let patch_area: u64 = plan
        .patches
        .iter()
        .map(|rect| rect.width as u64 * rect.height as u64)
        .sum();
    (move_area + patch_area) as f32 / total
}

#[cfg(windows)]
pub fn encode_keyframe_payload(
    frame_id: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
    jpeg_quality: u8,
) -> Result<Vec<u8>> {
    let jpeg = encode_jpeg_rgba(width, height, rgba, jpeg_quality)?;
    let mut payload = Vec::with_capacity(17 + jpeg.len());
    payload.extend_from_slice(&frame_id.to_be_bytes());
    payload.extend_from_slice(&width.to_be_bytes());
    payload.extend_from_slice(&height.to_be_bytes());
    payload.push(1);
    payload.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
    payload.extend_from_slice(&jpeg);
    Ok(payload)
}

#[cfg(windows)]
pub fn encode_delta_payload(
    frame_id: u32,
    base_frame_id: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
    plan: &DeltaPlan,
    jpeg_quality: u8,
) -> Result<Vec<u8>> {
    let mut patches = Vec::with_capacity(plan.patches.len());
    let mut total_patch_bytes = 0usize;
    for rect in &plan.patches {
        let patch = encode_patch_jpeg(width, height, rgba, *rect, jpeg_quality)?;
        total_patch_bytes += patch.len();
        patches.push((*rect, patch));
    }

    let mut payload = Vec::with_capacity(20 + plan.moves.len() * 24 + plan.patches.len() * 21 + total_patch_bytes);
    payload.extend_from_slice(&frame_id.to_be_bytes());
    payload.extend_from_slice(&base_frame_id.to_be_bytes());
    payload.extend_from_slice(&width.to_be_bytes());
    payload.extend_from_slice(&height.to_be_bytes());
    payload.extend_from_slice(&(plan.moves.len() as u16).to_be_bytes());
    payload.extend_from_slice(&(patches.len() as u16).to_be_bytes());
    for rect in &plan.moves {
        payload.extend_from_slice(&rect.src_x.to_be_bytes());
        payload.extend_from_slice(&rect.src_y.to_be_bytes());
        payload.extend_from_slice(&rect.dst_x.to_be_bytes());
        payload.extend_from_slice(&rect.dst_y.to_be_bytes());
        payload.extend_from_slice(&rect.width.to_be_bytes());
        payload.extend_from_slice(&rect.height.to_be_bytes());
    }
    for (rect, patch) in patches {
        payload.extend_from_slice(&(rect.x as i32).to_be_bytes());
        payload.extend_from_slice(&(rect.y as i32).to_be_bytes());
        payload.extend_from_slice(&rect.width.to_be_bytes());
        payload.extend_from_slice(&rect.height.to_be_bytes());
        payload.push(1);
        payload.extend_from_slice(&(patch.len() as u32).to_be_bytes());
        payload.extend_from_slice(&patch);
    }
    Ok(payload)
}

#[cfg(windows)]
fn encode_patch_jpeg(width: u32, height: u32, rgba: &[u8], rect: DirtyRect, jpeg_quality: u8) -> Result<Vec<u8>> {
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| anyhow!("Failed to build image buffer"))?;
    let patch = DynamicImage::ImageRgba8(image).crop_imm(rect.x, rect.y, rect.width, rect.height);

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut cursor, jpeg_quality.clamp(1, 100));
        encoder.encode_image(&patch)?;
    }
    Ok(cursor.into_inner())
}

#[cfg(windows)]
fn encode_jpeg_rgba(width: u32, height: u32, rgba: &[u8], jpeg_quality: u8) -> Result<Vec<u8>> {
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| anyhow!("Failed to build image buffer"))?;
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut cursor, jpeg_quality.clamp(1, 100));
        encoder.encode_image(&DynamicImage::ImageRgba8(image))?;
    }
    Ok(cursor.into_inner())
}

#[cfg(windows)]
fn tile_changed(
    previous: &[u8],
    current: &[u8],
    width: u32,
    x: u32,
    y: u32,
    tile_width: u32,
    tile_height: u32,
) -> bool {
    for row in y..(y + tile_height) {
        let start = ((row * width + x) * 4) as usize;
        let end = start + (tile_width * 4) as usize;
        if previous[start..end] != current[start..end] {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn hash_tile(rgba: &[u8], width: u32, rect: DirtyRect) -> u64 {
    let mut hash = 1469598103934665603u64;
    for row in rect.y..(rect.y + rect.height) {
        let start = ((row * width + rect.x) * 4) as usize;
        let end = start + (rect.width * 4) as usize;
        for byte in &rgba[start..end] {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(1099511628211u64);
        }
    }
    hash
}

#[cfg(windows)]
fn merge_dirty_rects(mut rects: Vec<DirtyRect>) -> Vec<DirtyRect> {
    rects.sort_by_key(|rect| (rect.y, rect.x));
    let mut changed = true;
    while changed {
        changed = false;
        let mut merged = Vec::with_capacity(rects.len());
        while !rects.is_empty() {
            let mut current = rects.remove(0);
            let mut index = 0;
            while index < rects.len() {
                if let Some(next) = merge_patch_pair(current, rects[index]) {
                    current = next;
                    rects.remove(index);
                    changed = true;
                    index = 0;
                } else {
                    index += 1;
                }
            }
            merged.push(current);
        }
        rects = merged;
    }
    rects
}

#[cfg(windows)]
fn merge_move_rects(mut rects: Vec<MoveRect>) -> Vec<MoveRect> {
    rects.sort_by_key(|rect| (rect.dst_y, rect.dst_x, rect.src_y, rect.src_x));
    let mut changed = true;
    while changed {
        changed = false;
        let mut merged = Vec::with_capacity(rects.len());
        while !rects.is_empty() {
            let mut current = rects.remove(0);
            let mut index = 0;
            while index < rects.len() {
                if let Some(next) = merge_move_pair(current, rects[index]) {
                    current = next;
                    rects.remove(index);
                    changed = true;
                    index = 0;
                } else {
                    index += 1;
                }
            }
            merged.push(current);
        }
        rects = merged;
    }
    rects
}

#[cfg(windows)]
fn merge_patch_pair(a: DirtyRect, b: DirtyRect) -> Option<DirtyRect> {
    let same_row_band = a.y == b.y && a.height == b.height;
    let horizontally_adjacent = a.x + a.width == b.x || b.x + b.width == a.x;
    if same_row_band && horizontally_adjacent {
        let left = a.x.min(b.x);
        let right = (a.x + a.width).max(b.x + b.width);
        return Some(DirtyRect {
            x: left,
            y: a.y,
            width: right - left,
            height: a.height,
        });
    }

    let same_column_band = a.x == b.x && a.width == b.width;
    let vertically_adjacent = a.y + a.height == b.y || b.y + b.height == a.y;
    if same_column_band && vertically_adjacent {
        let top = a.y.min(b.y);
        let bottom = (a.y + a.height).max(b.y + b.height);
        return Some(DirtyRect {
            x: a.x,
            y: top,
            width: a.width,
            height: bottom - top,
        });
    }

    None
}

#[cfg(windows)]
fn merge_move_pair(a: MoveRect, b: MoveRect) -> Option<MoveRect> {
    let same_delta_x = a.dst_x - a.src_x == b.dst_x - b.src_x;
    let same_delta_y = a.dst_y - a.src_y == b.dst_y - b.src_y;
    if !(same_delta_x && same_delta_y) {
        return None;
    }

    let same_row_band = a.dst_y == b.dst_y && a.height == b.height && a.src_y == b.src_y;
    let horizontally_adjacent = a.dst_x + a.width as i32 == b.dst_x || b.dst_x + b.width as i32 == a.dst_x;
    if same_row_band && horizontally_adjacent {
        let left_dst = a.dst_x.min(b.dst_x);
        let left_src = a.src_x.min(b.src_x);
        let right_dst = (a.dst_x + a.width as i32).max(b.dst_x + b.width as i32);
        return Some(MoveRect {
            src_x: left_src,
            src_y: a.src_y,
            dst_x: left_dst,
            dst_y: a.dst_y,
            width: (right_dst - left_dst) as u32,
            height: a.height,
        });
    }

    let same_column_band = a.dst_x == b.dst_x && a.width == b.width && a.src_x == b.src_x;
    let vertically_adjacent = a.dst_y + a.height as i32 == b.dst_y || b.dst_y + b.height as i32 == a.dst_y;
    if same_column_band && vertically_adjacent {
        let top_dst = a.dst_y.min(b.dst_y);
        let top_src = a.src_y.min(b.src_y);
        let bottom_dst = (a.dst_y + a.height as i32).max(b.dst_y + b.height as i32);
        return Some(MoveRect {
            src_x: a.src_x,
            src_y: top_src,
            dst_x: a.dst_x,
            dst_y: top_dst,
            width: a.width,
            height: (bottom_dst - top_dst) as u32,
        });
    }

    None
}
