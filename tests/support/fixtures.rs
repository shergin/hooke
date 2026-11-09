//! Reader for the Chrome fixture corpus in `tests/fixtures/` (format: `tests/fixtures/README.md`).

use super::{ahem_text, view, Node};
use hooke::{
    AlignContent, AlignItems, Available, BoxSizing, Direction, Display, Edges, FlexDirection, FlexWrap, Handle,
    JustifyContent, Length, Opt, Overflow, Position, Size, Style,
};

/// One node of a fixture: its style, its Ahem text if it is a text leaf, and Chrome's frame.
#[derive(Clone, Debug)]
pub struct FixtureNode {
    pub style: Style,
    pub text: Option<String>,
    /// Chrome's `x y width height`: the position is relative to the node's containing block
    /// (the parent, or for `position: absolute` the nearest positioned ancestor, or the root),
    /// the root's relative to the viewport.
    pub expected: [f32; 4],
    /// The `key=value` part of the node's line, for diagnostics.
    pub props: String,
    pub children: Vec<FixtureNode>,
}

#[derive(Clone, Debug)]
pub struct Fixture {
    pub source: String,
    pub available: Size<Available<f32>>,
    pub rounding: bool,
    pub root: FixtureNode,
}

fn length(key: &str, v: &str) -> Result<Length<f32>, String> {
    Ok(match v {
        "auto" => Length::Auto,
        "none" => Length::None,
        "min-content" => Length::MinContent,
        "max-content" => Length::MaxContent,
        "fit-content" => Length::FitContent,
        "stretch" => Length::Stretch,
        "content" => Length::Content,
        _ => {
            if let Some(n) = v.strip_suffix("px") {
                Length::Px(n.parse::<f32>().map_err(|e| format!("{key}={v}: {e}"))?)
            } else if let Some(n) = v.strip_suffix('%') {
                // Percentages are fractions in hooke.
                Length::Percent((n.parse::<f64>().map_err(|e| format!("{key}={v}: {e}"))? / 100.0) as f32)
            } else {
                return Err(format!("{key}={v}: not a length"));
            }
        }
    })
}

fn px(key: &str, v: &str) -> Result<f32, String> {
    v.strip_suffix("px").and_then(|n| n.parse().ok()).ok_or_else(|| format!("{key}={v}: not a px length"))
}

fn num(key: &str, v: &str) -> Result<f32, String> {
    v.parse().map_err(|e| format!("{key}={v}: {e}"))
}

fn align_items(key: &str, v: &str) -> Result<AlignItems, String> {
    Ok(match v {
        "normal" => AlignItems::Normal,
        "stretch" => AlignItems::Stretch,
        "flex-start" => AlignItems::FlexStart,
        "flex-end" => AlignItems::FlexEnd,
        "center" => AlignItems::Center,
        "baseline" => AlignItems::Baseline,
        "start" => AlignItems::Start,
        "end" => AlignItems::End,
        "self-start" => AlignItems::SelfStart,
        "self-end" => AlignItems::SelfEnd,
        _ => return Err(format!("{key}={v}")),
    })
}

fn justify_content(key: &str, v: &str) -> Result<JustifyContent, String> {
    Ok(match v {
        "normal" => JustifyContent::Normal,
        "flex-start" => JustifyContent::FlexStart,
        "flex-end" => JustifyContent::FlexEnd,
        "center" => JustifyContent::Center,
        "space-between" => JustifyContent::SpaceBetween,
        "space-around" => JustifyContent::SpaceAround,
        "space-evenly" => JustifyContent::SpaceEvenly,
        "start" => JustifyContent::Start,
        "end" => JustifyContent::End,
        "stretch" => JustifyContent::Stretch,
        _ => return Err(format!("{key}={v}")),
    })
}

fn align_content(key: &str, v: &str) -> Result<AlignContent, String> {
    Ok(match v {
        "normal" => AlignContent::Normal,
        "flex-start" => AlignContent::FlexStart,
        "flex-end" => AlignContent::FlexEnd,
        "center" => AlignContent::Center,
        "space-between" => AlignContent::SpaceBetween,
        "space-around" => AlignContent::SpaceAround,
        "space-evenly" => AlignContent::SpaceEvenly,
        "stretch" => AlignContent::Stretch,
        "start" => AlignContent::Start,
        "end" => AlignContent::End,
        _ => return Err(format!("{key}={v}")),
    })
}

fn overflow(key: &str, v: &str) -> Result<Overflow, String> {
    Ok(match v {
        "visible" => Overflow::Visible,
        "clip" => Overflow::Clip,
        "hidden" => Overflow::Hidden,
        "scroll" => Overflow::Scroll,
        _ => return Err(format!("{key}={v}")),
    })
}

fn unescape(v: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = v;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find('}').filter(|_| tail.starts_with("\\u{")).ok_or_else(|| format!("bad escape in {v}"))?;
        let code = u32::from_str_radix(&tail[3..end], 16).map_err(|e| format!("bad escape in {v}: {e}"))?;
        out.push(char::from_u32(code).ok_or_else(|| format!("bad escape in {v}"))?);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Builds a hooke style from a node line's `key=value` pairs, starting from the CSS initial
/// values. `position` and `box-sizing` must be present: Taffy's defaults differ from hooke's.
fn style(props: &str) -> Result<(Style, Option<String>), String> {
    let mut s = Style::web();
    let mut text = None;
    let (mut size, mut min, mut max) = ([Length::Auto; 2], [Length::Auto; 2], [Length::None; 2]);
    let mut gap = [Length::zero(); 2]; // row, column
    let mut margin = Edges::all(Length::zero());
    let mut padding = Edges::all(Length::zero());
    let mut inset = Edges::all(Length::Auto);
    let mut border = Edges::all(0.0f32);
    let (mut has_position, mut has_box_sizing) = (false, false);
    for pair in props.split_whitespace() {
        let (k, v) = pair.split_once('=').ok_or_else(|| format!("{pair}: expected key=value"))?;
        match k {
            "display" => {
                s = s.with_display(match v {
                    "flex" => Display::Flex,
                    "none" => Display::None,
                    _ => return Err(pair.to_string()),
                })
            }
            "position" => {
                has_position = true;
                s = s.with_position(match v {
                    "static" => Position::Static,
                    "relative" => Position::Relative,
                    "absolute" => Position::Absolute,
                    _ => return Err(pair.to_string()),
                })
            }
            "box-sizing" => {
                has_box_sizing = true;
                s = s.with_box_sizing(match v {
                    "border-box" => BoxSizing::BorderBox,
                    "content-box" => BoxSizing::ContentBox,
                    _ => return Err(pair.to_string()),
                })
            }
            "direction" => {
                s = s.with_direction(match v {
                    "ltr" => Direction::Ltr,
                    "rtl" => Direction::Rtl,
                    _ => return Err(pair.to_string()),
                })
            }
            "flex-direction" => {
                s = s.with_flex_direction(match v {
                    "row" => FlexDirection::Row,
                    "row-reverse" => FlexDirection::RowReverse,
                    "column" => FlexDirection::Column,
                    "column-reverse" => FlexDirection::ColumnReverse,
                    _ => return Err(pair.to_string()),
                })
            }
            "flex-wrap" => {
                s = s.with_flex_wrap(match v {
                    "nowrap" => FlexWrap::NoWrap,
                    "wrap" => FlexWrap::Wrap,
                    "wrap-reverse" => FlexWrap::WrapReverse,
                    _ => return Err(pair.to_string()),
                })
            }
            "overflow-x" => s = s.with_overflow_x(overflow(k, v)?),
            "overflow-y" => s = s.with_overflow_y(overflow(k, v)?),
            "scrollbar-width" => s = s.with_scrollbar_gutter(num(k, v)?),
            "align-items" => s = s.with_align_items(align_items(k, v)?),
            "align-self" => s = s.with_align_self(Some(align_items(k, v)?)),
            "align-content" => s = s.with_align_content(align_content(k, v)?),
            "justify-content" => s = s.with_justify_content(justify_content(k, v)?),
            "flex-grow" => s = s.with_flex_grow(num(k, v)?),
            "flex-shrink" => s = s.with_flex_shrink(num(k, v)?),
            "flex-basis" => s = s.with_flex_basis(length(k, v)?),
            "order" => s = s.with_order(v.parse().map_err(|e| format!("{pair}: {e}"))?),
            "width" => size[0] = length(k, v)?,
            "height" => size[1] = length(k, v)?,
            "min-width" => min[0] = length(k, v)?,
            "min-height" => min[1] = length(k, v)?,
            "max-width" => max[0] = length(k, v)?,
            "max-height" => max[1] = length(k, v)?,
            "aspect-ratio" => s = s.with_aspect_ratio(Opt::some(num(k, v)?)),
            "row-gap" => gap[0] = length(k, v)?,
            "column-gap" => gap[1] = length(k, v)?,
            "margin-top" => margin.top = length(k, v)?,
            "margin-right" => margin.right = length(k, v)?,
            "margin-bottom" => margin.bottom = length(k, v)?,
            "margin-left" => margin.left = length(k, v)?,
            "padding-top" => padding.top = length(k, v)?,
            "padding-right" => padding.right = length(k, v)?,
            "padding-bottom" => padding.bottom = length(k, v)?,
            "padding-left" => padding.left = length(k, v)?,
            "border-top" => border.top = px(k, v)?,
            "border-right" => border.right = px(k, v)?,
            "border-bottom" => border.bottom = px(k, v)?,
            "border-left" => border.left = px(k, v)?,
            "top" => inset.top = length(k, v)?,
            "right" => inset.right = length(k, v)?,
            "bottom" => inset.bottom = length(k, v)?,
            "left" => inset.left = length(k, v)?,
            "text" => text = Some(unescape(v)?),
            _ => return Err(format!("{pair}: unknown property")),
        }
    }
    if !has_position || !has_box_sizing {
        return Err("position and box-sizing must be explicit".into());
    }
    let s = s
        .with_size(size[0], size[1])
        .with_min_size(min[0], min[1])
        .with_max_size(max[0], max[1])
        .with_gap(gap[0], gap[1])
        .with_margin(margin)
        .with_padding(padding)
        .with_inset(inset)
        .with_border(border);
    Ok((s, text))
}

fn available(key: &str, v: &str) -> Result<Available<f32>, String> {
    Ok(match v {
        "max-content" => Available::MaxContent,
        "min-content" => Available::MinContent,
        _ => Available::Definite(px(key, v)?),
    })
}

/// Parses one fixture file.
pub fn parse(src: &str) -> Result<Fixture, String> {
    let mut source = String::new();
    let mut avail = None;
    let mut rounding = None;
    // Stack of (depth, node) for the nodes whose children are still being read.
    let mut stack: Vec<(usize, FixtureNode)> = Vec::new();
    let mut root = None;
    for (n, line) in src.lines().enumerate() {
        let at = |e: String| format!("line {}: {e}", n + 1);
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        let mut words = trimmed.splitn(2, ' ');
        let head = words.next().unwrap_or("");
        let rest = words.next().unwrap_or("");
        match head {
            "source" => source = rest.to_string(),
            "viewport" => {
                let (w, h) = rest.split_once(' ').ok_or_else(|| at("viewport: expected two values".into()))?;
                avail = Some(Size::new(available("viewport", w).map_err(at)?, available("viewport", h).map_err(at)?));
            }
            "rounding" => rounding = Some(rest == "true"),
            "node" => {
                if indent % 2 != 0 {
                    return Err(at("odd indentation".into()));
                }
                let depth = indent / 2;
                let mut parts = rest.splitn(5, ' ');
                let mut expected = [0.0f32; 4];
                for e in expected.iter_mut() {
                    *e = num("frame", parts.next().unwrap_or("")).map_err(at)?;
                }
                let props = parts.next().unwrap_or("").to_string();
                let (style, text) = style(&props).map_err(at)?;
                let node = FixtureNode { style, text, expected, props, children: Vec::new() };
                // Close every open node at this depth or deeper.
                while stack.last().is_some_and(|(d, _)| *d >= depth) {
                    let (_, done) = stack.pop().unwrap();
                    match stack.last_mut() {
                        Some((_, parent)) => parent.children.push(done),
                        None => root = Some(done),
                    }
                }
                if depth != stack.len() || (depth == 0 && root.is_some()) {
                    return Err(at("unexpected indentation".into()));
                }
                stack.push((depth, node));
            }
            _ => return Err(at(format!("unknown line {head:?}"))),
        }
    }
    while let Some((_, done)) = stack.pop() {
        match stack.last_mut() {
            Some((_, parent)) => parent.children.push(done),
            None => root = Some(done),
        }
    }
    Ok(Fixture {
        source,
        available: avail.ok_or("missing viewport")?,
        rounding: rounding.ok_or("missing rounding")?,
        root: root.ok_or("no nodes")?,
    })
}

/// Builds the host tree. Tags are pre-order indices. A leaf with text is an Ahem text leaf; any
/// other node, including an empty leaf, is a view (an empty container).
pub fn build(node: &FixtureNode, next_tag: &mut u32) -> Handle<Node> {
    let tag = *next_tag;
    *next_tag += 1;
    match &node.text {
        Some(text) if node.children.is_empty() => ahem_text(tag, node.style, text, false),
        _ => view(tag, node.style, node.children.iter().map(|c| build(c, next_tag)).collect()),
    }
}
