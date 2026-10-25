//! Responsive layout. The same three areas at every size; what changes is how many are on screen.
//!
//! - 140+ columns: navigator, inspector and workspace side by side.
//! - 100..139: the same, with narrower side columns.
//! - 61..99: the side areas collapse and open docked beside the workspace.
//! - 60 and below: one area at a time, switched from a strip.

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Full,
    Reduced,
    Collapsed,
    Single,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Area {
    Navigator,
    Inspector,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visibility {
    pub navigator: bool,
    pub inspector: bool,
    /// Which area is shown in single-pane mode, or opened as a drawer in collapsed mode.
    pub active: Area,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub mode: Mode,
    pub header: Option<Rect>,
    pub navigator: Option<Rect>,
    pub inspector: Option<Rect>,
    pub main: Rect,
    pub footer: Option<Rect>,
}

pub fn mode_for(width: u16) -> Mode {
    match width {
        140.. => Mode::Full,
        100..=139 => Mode::Reduced,
        61..=99 => Mode::Collapsed,
        _ => Mode::Single,
    }
}

fn split(rect: Rect, left: u16) -> (Rect, Rect) {
    let left = left.min(rect.width);
    (
        Rect::new(rect.x, rect.y, left, rect.height),
        Rect::new(rect.x + left, rect.y, rect.width - left, rect.height),
    )
}

pub fn compute(area: Rect, visible: Visibility) -> Frame {
    let mode = mode_for(area.width);
    // One row of header and one of key hints; a very short terminal gives the room to the content.
    let (header_rows, footer_rows) = if area.height >= 12 {
        (1, 1)
    } else if area.height >= 6 {
        (0, 1)
    } else {
        (0, 0)
    };
    let header = (header_rows > 0).then(|| Rect::new(area.x, area.y, area.width, 1));
    let footer =
        (footer_rows > 0).then(|| Rect::new(area.x, area.y + area.height - 1, area.width, 1));
    let body = Rect::new(
        area.x,
        area.y + header_rows,
        area.width,
        area.height.saturating_sub(header_rows + footer_rows),
    );

    let (navigator, inspector, main) = match mode {
        Mode::Full | Mode::Reduced => {
            let (nav_width, inspector_width) = if mode == Mode::Full {
                (30, 38)
            } else {
                (24, 28)
            };
            let mut rest = body;
            let mut navigator = None;
            let mut inspector = None;
            if visible.navigator {
                let (left, right) = split(rest, nav_width);
                navigator = Some(left);
                rest = right;
            }
            if visible.inspector {
                let main_width = rest.width.saturating_sub(inspector_width);
                let (left, right) = split(rest, main_width);
                inspector = Some(right);
                rest = left;
            }
            (navigator, inspector, rest)
        }
        Mode::Collapsed => {
            // Closed, the workspace has the whole width. Opened, a side area docks beside it
            // (never floats over it, which would leave half a panel visible).
            let drawer_width = 30.min(body.width.saturating_sub(24));
            match (visible.active, visible.navigator, visible.inspector) {
                (Area::Navigator, true, _) => {
                    let (left, right) = split(body, drawer_width);
                    (Some(left), None, right)
                }
                (Area::Inspector, _, true) => {
                    let (left, right) = split(body, body.width - drawer_width);
                    (None, Some(right), left)
                }
                _ => (None, None, body),
            }
        }
        Mode::Single => match visible.active {
            Area::Navigator => (Some(body), None, Rect::default()),
            Area::Inspector => (None, Some(body), Rect::default()),
            Area::Main => (None, None, body),
        },
    };
    Frame {
        mode,
        header,
        navigator,
        inspector,
        main,
        footer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u16, height: u16, navigator: bool, inspector: bool, active: Area) -> Frame {
        compute(
            Rect::new(0, 0, width, height),
            Visibility {
                navigator,
                inspector,
                active,
            },
        )
    }

    #[test]
    fn full_size_shows_all_three_areas_without_overlap() {
        let f = frame(160, 45, true, true, Area::Main);
        assert_eq!(f.mode, Mode::Full);
        let (nav, ins) = (f.navigator.unwrap(), f.inspector.unwrap());
        assert_eq!((nav.width, ins.width), (30, 38));
        assert_eq!(nav.width + f.main.width + ins.width, 160);
        assert!(nav.right() <= f.main.x && f.main.right() <= ins.x);
        assert_eq!((f.header.unwrap().height, f.footer.unwrap().y), (1, 44));
    }

    #[test]
    fn reduced_width_keeps_three_areas_with_narrower_columns() {
        let f = frame(100, 30, true, true, Area::Main);
        assert_eq!(f.mode, Mode::Reduced);
        assert_eq!(
            (f.navigator.unwrap().width, f.inspector.unwrap().width),
            (24, 28)
        );
        assert_eq!(f.main.width, 48);
    }

    #[test]
    fn eighty_columns_collapse_side_areas_into_a_drawer() {
        let closed = frame(80, 24, true, true, Area::Main);
        assert_eq!(closed.mode, Mode::Collapsed);
        assert!(closed.navigator.is_none() && closed.inspector.is_none());
        assert_eq!(closed.main.width, 80);
        let drawer = frame(80, 24, true, true, Area::Navigator);
        assert_eq!(drawer.navigator.unwrap().width, 30);
        assert_eq!(
            drawer.main.width, 50,
            "the drawer docks; it never covers the workspace"
        );
        assert_eq!(drawer.navigator.unwrap().right(), drawer.main.x);
    }

    #[test]
    fn sixty_columns_and_below_show_one_area_at_a_time() {
        for (area, nav, ins) in [
            (Area::Navigator, true, false),
            (Area::Inspector, false, true),
            (Area::Main, false, false),
        ] {
            let f = frame(60, 24, true, true, area);
            assert_eq!(f.mode, Mode::Single);
            assert_eq!((f.navigator.is_some(), f.inspector.is_some()), (nav, ins));
        }
        assert_eq!(mode_for(40), Mode::Single);
    }

    #[test]
    fn tiny_terminals_never_underflow() {
        for (w, h) in [(1, 1), (10, 3), (20, 5), (59, 6), (200, 2)] {
            let f = frame(w, h, true, true, Area::Main);
            assert!(f.main.right() <= w && f.main.bottom() <= h, "{w}x{h}");
        }
    }
}
