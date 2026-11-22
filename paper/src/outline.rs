use core::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// Trace the grid line and its outside pixel clockwise in cursor order.
/// Keep the full perimeter here so clipping never shifts the color boundaries.
pub(crate) fn segments(
    (x, y, width, height): (u32, u32, u32, u32),
    page: (u32, u32),
    shares: usize,
) -> impl Iterator<Item = (usize, Rect)> {
    let last_x = page.0.saturating_sub(1);
    let last_y = page.1.saturating_sub(1);
    let left = i64::from(x.min(last_x));
    let top = i64::from(y.min(last_y));
    let right = i64::from(x.saturating_add(width).min(last_x));
    let bottom = i64::from(y.saturating_add(height).min(last_y));
    let inner = sides(left, top, right, bottom);
    let outer = sides(left - 1, top - 1, right + 1, bottom + 1);
    let inner_len = inner.iter().map(|side| side.length).sum::<i64>();
    let total_len = inner_len + outer.iter().map(|side| side.length).sum::<i64>();
    let shares = if page.0 == 0 || page.1 == 0 {
        0
    } else {
        shares.min(usize::try_from(total_len).unwrap_or(usize::MAX))
    };

    [(inner, false), (outer, true)]
        .into_iter()
        .flat_map(move |(sides, outside)| {
            let mut offset = 0;
            sides.into_iter().flat_map(move |side| {
                let origin = offset;
                offset += side.length;
                let boundary = move |share: usize| {
                    let split =
                        |length: i64| (i128::from(length) * share as i128 / shares as i128) as i64;
                    if outside {
                        // Keep every share within one pixel of ideal on the full band.
                        split(total_len) - split(inner_len)
                    } else {
                        split(inner_len)
                    }
                };
                (0..shares).filter_map(move |share| {
                    let start = (boundary(share) - origin).max(0);
                    let end = (boundary(share + 1) - origin).min(side.length);
                    side.span(start..end).map(|rect| (share, rect))
                })
            })
        })
}

#[derive(Clone, Copy)]
struct Side {
    x: i64,
    y: i64,
    dx: i64,
    dy: i64,
    length: i64,
}

impl Side {
    const fn new(x: i64, y: i64, dx: i64, dy: i64, length: i64) -> Self {
        Self {
            x,
            y,
            dx,
            dy,
            length,
        }
    }

    fn span(self, range: Range<i64>) -> Option<Rect> {
        if range.is_empty() {
            return None;
        }
        let first = range.start;
        let last = range.end - 1;
        Some(Rect {
            x: self.x + (first * self.dx).min(last * self.dx),
            y: self.y + (first * self.dy).min(last * self.dy),
            width: 1 + (last - first) * self.dx.abs(),
            height: 1 + (last - first) * self.dy.abs(),
        })
    }
}

fn sides(left: i64, top: i64, right: i64, bottom: i64) -> [Side; 4] {
    let width = right - left;
    let height = bottom - top;
    let empty = Side::new(0, 0, 0, 0, 0);
    // Compact sheets can snap both edges of a cell to a single pixel.
    if width == 0 {
        return [Side::new(left, top, 0, 1, height + 1), empty, empty, empty];
    }
    if height == 0 {
        return [Side::new(left, top, 1, 0, width + 1), empty, empty, empty];
    }
    // Each side owns its first corner so no pixel is counted twice.
    [
        Side::new(left, top, 1, 0, width),
        Side::new(right, top, 0, 1, height),
        Side::new(right, bottom, -1, 0, width),
        Side::new(left, bottom, 0, -1, height),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    fn pixels(
        bounds: (u32, u32, u32, u32),
        page: (u32, u32),
        shares: usize,
    ) -> BTreeMap<(i64, i64), usize> {
        let mut pixels = BTreeMap::new();
        for (share, rect) in segments(bounds, page, shares) {
            for y in rect.y..rect.y + rect.height {
                for x in rect.x..rect.x + rect.width {
                    assert!(pixels.insert((x, y), share).is_none());
                }
            }
        }
        pixels
    }

    #[test]
    fn outline_replaces_the_grid_line_and_covers_the_outer_corners() {
        let pixels = pixels((10, 20, 4, 3), (100, 100), 1);
        for y in 18..=25 {
            for x in 8..=16 {
                let outer = (9..=15).contains(&x) && (19..=24).contains(&y);
                let interior = (11..14).contains(&x) && (21..23).contains(&y);
                assert_eq!(
                    pixels.get(&(x, y)).copied(),
                    (outer && !interior).then_some(0)
                );
            }
        }
    }

    #[test]
    fn shares_partition_both_rings_clockwise_and_evenly() {
        let pixels = pixels((10, 20, 7, 4), (100, 100), 3);
        let mut counts = [0; 3];
        for &share in pixels.values() {
            counts[share] += 1;
        }
        assert_eq!(counts, [17, 17, 18]);
        for ((x, y), share) in [
            ((10, 20), 0),
            ((17, 20), 1),
            ((10, 24), 2),
            ((9, 19), 0),
            ((18, 19), 0),
            ((18, 20), 1),
            ((9, 25), 2),
        ] {
            assert_eq!(pixels[&(x, y)], share);
        }
    }

    #[test]
    fn compact_outline_aligns_to_the_last_grid_pixel() {
        let pixels = pixels((0, 0, 4, 3), (4, 3), 1);
        assert!(pixels.contains_key(&(-1, -1)));
        assert!(pixels.contains_key(&(3, 2)));
        assert!(pixels.contains_key(&(4, 3)));
        assert!(!pixels.contains_key(&(5, 4)));
        assert!(!pixels.contains_key(&(2, 1)));
    }
}
