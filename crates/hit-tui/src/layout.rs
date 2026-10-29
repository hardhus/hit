//! Where things go on screen, separate from what is drawn there.
//!
//! Screens render into the [`Regions`] they are given and never decide the
//! arrangement themselves, so other layouts (a sidebar, full-screen panes)
//! can be added later without touching screens (audit D-036).

use ratatui::layout::{Constraint, Layout, Rect};

/// The areas of the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Regions {
    pub tabs: Rect,
    pub list: Rect,
    /// `None` when the terminal is too narrow for a side preview.
    pub preview: Option<Rect>,
    pub status: Rect,
    pub hints: Rect,
}

pub trait LayoutStrategy: Send {
    fn regions(&self, area: Rect) -> Regions;
}

/// Tabs on top, list with a preview on the right when wide enough, status
/// and hints at the bottom.
#[derive(Debug, Clone, Copy)]
pub struct TabsLayout {
    /// Minimum width for showing the preview beside the list.
    pub preview_min_width: u16,
    /// Share of the width given to the list (percent).
    pub list_percent: u16,
}

impl Default for TabsLayout {
    fn default() -> Self {
        Self {
            preview_min_width: 110,
            list_percent: 52,
        }
    }
}

impl LayoutStrategy for TabsLayout {
    fn regions(&self, area: Rect) -> Regions {
        let [tabs, body, status, hints] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let (list, preview) = if area.width >= self.preview_min_width {
            let [list, _, preview] = Layout::horizontal([
                Constraint::Percentage(self.list_percent),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(body);
            (list, Some(preview))
        } else {
            (body, None)
        };
        Regions {
            tabs,
            list,
            preview,
            status,
            hints,
        }
    }
}

/// A centered box of at most `width` × `height` inside `area`.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_only_when_wide() {
        let layout = TabsLayout::default();
        let wide = layout.regions(Rect::new(0, 0, 160, 40));
        assert!(wide.preview.is_some());
        assert_eq!(wide.tabs.height, 2);
        assert_eq!(wide.hints.y, 39);
        assert_eq!(wide.status.y, 38);
        let narrow = layout.regions(Rect::new(0, 0, 80, 24));
        assert!(narrow.preview.is_none());
        assert_eq!(narrow.list.width, 80);
    }

    #[test]
    fn centered_box_fits() {
        let area = Rect::new(0, 0, 50, 10);
        assert_eq!(centered(area, 20, 4), Rect::new(15, 3, 20, 4));
        assert_eq!(centered(area, 200, 40), area);
    }
}
