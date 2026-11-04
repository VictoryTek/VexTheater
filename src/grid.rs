//! The tiles of a page, laid out in a grid that wraps onto more rows instead of running
//! off the screen or scrolling. Sizes are logical pixels; GNOME's display scale handles 4K.

use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub const PAGE_MARGIN: i32 = 96;
pub const TILE_SPACING: i32 = 40;
/// Tiles are this big when the screen has room, and shrink to fit it.
pub const TILE_MAX: i32 = 200;
/// The size a tile asks for until the window's size is known: a larger minimum would
/// make the window wider than a small screen.
pub const TILE_MIN: i32 = 100;
/// The least gap between tiles, wide enough for a focused tile, which is drawn larger,
/// not to cover its neighbours much.
pub const TILE_MIN_SPACING: i32 = 16;
pub const ICON_SIZE: i32 = 96;
/// Room kept under what a page measures for its other parts, since a label's height
/// changes with its text.
const CHROME_SLACK: i32 = 40;

/// The gap between tiles of side `size`: [`TILE_SPACING`] for full-size tiles, closing up
/// as they shrink, but never below [`TILE_MIN_SPACING`] (or the tile itself, for tiny ones).
fn spacing_for(size: i32) -> i32 {
    (size / 5).clamp(TILE_MIN_SPACING.min(size), TILE_SPACING)
}

/// The side of the square tiles, the gap between them, and how many fit in a row, for
/// `count` tiles in a window `width` wide with `height` of room for them. Tiles are
/// [`TILE_MAX`] if that fits, as many to a row as the width holds, wrapping onto more
/// rows; if those rows are too tall, tiles shrink (so more fit in a row) until they fit.
/// The grid is always inside `width` and `height`, down to tiles one pixel wide.
pub fn grid_layout(width: i32, height: i32, count: usize) -> (i32, i32, usize) {
    let room_width = (width - 2 * PAGE_MARGIN).max(1);
    let room_height = height.max(1);
    let count = count.max(1);
    let fit = |size: i32| {
        let spacing = spacing_for(size);
        let columns = usize::try_from((room_width + spacing) / (size + spacing))
            .unwrap_or(1)
            .max(1);
        let rows = i32::try_from(count.div_ceil(columns)).unwrap_or(i32::MAX);
        let used = rows.saturating_mul(size) + (rows - 1).saturating_mul(spacing);
        (spacing, columns, used <= room_height)
    };
    for size in (1..=TILE_MAX).rev() {
        let (spacing, columns, fits) = fit(size);
        if fits {
            return (size, spacing, columns);
        }
    }
    let (spacing, columns, _) = fit(1);
    (1, spacing, columns)
}

/// Where a tile at `position` of `len` goes when moved one row up (`rows` is -1) or down
/// (+1) in a grid `columns` wide, or none: not from the top row up, nor from the last row
/// down. Down from a row that has no tile below goes to the last tile, so a short last
/// row can still be reached.
pub fn step_row(position: usize, rows: isize, columns: usize, len: usize) -> Option<usize> {
    let columns = columns.max(1);
    if rows < 0 {
        return position.checked_sub(columns);
    }
    let last_row = len.saturating_sub(1) / columns;
    (position / columns < last_row).then(|| (position + columns).min(len - 1))
}

/// The tiles of one page: the grid and the order they are shown in. The buttons stay in
/// the grid when they change places (only their cell changes), so moving a tile keeps
/// the keyboard focus on it.
pub struct TileGrid {
    /// The page the grid is on, to measure what else is on it.
    column: gtk::Box,
    grid: gtk::Grid,
    /// In display order.
    tiles: RefCell<Vec<gtk::Button>>,
    columns: Cell<usize>,
}

impl TileGrid {
    /// A grid for `column`, which the caller puts it in, between the page's other parts.
    pub fn new(column: &gtk::Box) -> Rc<TileGrid> {
        let grid = gtk::Grid::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .vexpand(true)
            .column_spacing(TILE_SPACING)
            .row_spacing(TILE_SPACING)
            .build();
        Rc::new(TileGrid {
            column: column.clone(),
            grid,
            tiles: RefCell::default(),
            // One row, until the window's size is known.
            columns: Cell::new(usize::MAX),
        })
    }

    pub fn widget(&self) -> &gtk::Grid {
        &self.grid
    }

    /// Adds a tile after the others.
    pub fn push(&self, button: &gtk::Button) {
        let (column, row) = self.cell(self.tiles.borrow().len());
        self.grid.attach(button, column, row, 1, 1);
        self.tiles.borrow_mut().push(button.clone());
    }

    /// The tiles in display order.
    pub fn buttons(&self) -> Vec<gtk::Button> {
        self.tiles.borrow().clone()
    }

    pub fn first(&self) -> Option<gtk::Button> {
        self.tiles.borrow().first().cloned()
    }

    pub fn position(&self, button: &gtk::Button) -> Option<usize> {
        self.tiles.borrow().iter().position(|tile| tile == button)
    }

    /// The row of the grid `button` is in.
    pub fn row_of(&self, button: &gtk::Button) -> Option<usize> {
        self.position(button)
            .map(|position| self.cell(position).1 as usize)
    }

    pub fn columns(&self) -> usize {
        self.columns.get()
    }

    pub fn len(&self) -> usize {
        self.tiles.borrow().len()
    }

    /// Moves `button` to `index`, the others keeping their order, and lays the tiles out
    /// again.
    pub fn move_to(&self, button: &gtk::Button, index: usize) {
        {
            let mut tiles = self.tiles.borrow_mut();
            let Some(from) = tiles.iter().position(|tile| tile == button) else {
                return;
            };
            let tile = tiles.remove(from);
            let end = tiles.len();
            tiles.insert(index.min(end), tile);
        }
        self.arrange();
    }

    /// Shows the tiles in `order`, which holds the same tiles as now.
    pub fn set_order(&self, order: Vec<gtk::Button>) {
        if order.len() == self.len() {
            *self.tiles.borrow_mut() = order;
            self.arrange();
        }
    }

    /// The (column, row) of the tile shown at `position`.
    fn cell(&self, position: usize) -> (i32, i32) {
        let columns = self.columns.get().max(1);
        ((position % columns) as i32, (position / columns) as i32)
    }

    /// Puts every tile in its cell.
    fn arrange(&self) {
        let layout = self.grid.layout_manager().and_downcast::<gtk::GridLayout>();
        for (position, button) in self.tiles.borrow().iter().enumerate() {
            let (column, row) = self.cell(position);
            if let Some(child) = layout
                .as_ref()
                .map(|layout| layout.layout_child(button))
                .and_then(|child| child.downcast::<gtk::GridLayoutChild>().ok())
            {
                child.set_column(column);
                child.set_row(row);
            }
        }
    }

    /// What the page takes besides the grid: its other parts and margins.
    fn chrome(&self) -> i32 {
        let mut total = self.column.margin_top() + self.column.margin_bottom();
        let mut child = self.column.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if widget.upcast_ref::<gtk::Widget>() != self.grid.upcast_ref::<gtk::Widget>() {
                total += widget.measure(gtk::Orientation::Vertical, -1).1;
            }
        }
        total
    }

    /// Sizes and places the tiles for a window `width` by `height` logical pixels, so
    /// they all show.
    pub fn fit(&self, width: i32, height: i32) {
        let room = height - self.chrome() - CHROME_SLACK;
        let (size, spacing, columns) = grid_layout(width, room, self.len());
        self.columns.set(columns);
        self.grid.set_row_spacing(spacing as u32);
        self.grid.set_column_spacing(spacing as u32);
        for tile in self.tiles.borrow().iter() {
            tile.set_size_request(size, size);
            if let Some(icon) = tile
                .child()
                .and_then(|child| child.downcast::<gtk::Image>().ok())
            {
                icon.set_pixel_size(size * ICON_SIZE / TILE_MAX);
            }
        }
        self.arrange();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The (width, height) a grid of `count` tiles takes in a layout.
    fn extent(width: i32, height: i32, count: usize) -> (i32, i32) {
        let (size, spacing, columns) = grid_layout(width, height, count);
        let rows = count.div_ceil(columns) as i32;
        let across = columns.min(count) as i32;
        (
            across * size + (across - 1) * spacing,
            rows * size + (rows - 1) * spacing,
        )
    }

    // The windows the launcher is shown in: logical sizes, after the display scale.
    // 1920x1080 at scale 1, and 3840x2160 at scale 2, are the same 1920x1080.
    const SIZES: [(i32, i32); 4] = [(1280, 720), (1366, 768), (1920, 1080), (3840, 2160)];

    #[test]
    fn every_tile_is_inside_the_window_whatever_the_count() {
        for (width, height) in SIZES {
            // What a page leaves for the tiles: its header, labels and hints take some.
            let room = height - 450;
            for count in [1, 2, 6, 7, 8, 20, 60] {
                let (across, down) = extent(width, room, count);
                assert!(
                    across <= width - 2 * PAGE_MARGIN,
                    "{count} at {width}: {across}"
                );
                assert!(down <= room, "{count} at {width}x{room}: {down}");
            }
        }
    }

    #[test]
    fn one_tile_and_a_few_keep_full_size_in_one_row() {
        for (width, height) in [(1920, 1080), (3840, 2160)] {
            for count in [1, 6] {
                let (size, spacing, columns) = grid_layout(width, height - 450, count);
                assert_eq!(
                    (size, spacing),
                    (TILE_MAX, TILE_SPACING),
                    "{count} at {width}"
                );
                assert!(columns >= count, "{count} at {width}: {columns} to a row");
            }
        }
        // 1920 wide holds exactly seven.
        assert_eq!(grid_layout(1920, 630, 1).2, 7);
    }

    #[test]
    fn tiles_that_do_not_fit_one_row_wrap_instead_of_shrinking() {
        // 1920 wide holds 7 tiles of 200 with 40 between; 8 and 14 take two rows.
        for count in [8, 14] {
            let (size, spacing, columns) = grid_layout(1920, 630, count);
            assert_eq!(
                (size, spacing, columns),
                (TILE_MAX, TILE_SPACING, 7),
                "{count}"
            );
        }
        // And 15 to 21 take three rows, which no longer fit at full size in 630.
        let (size, _, columns) = grid_layout(1920, 630, 21);
        assert!(size < TILE_MAX && columns >= 7, "{size} {columns}");
    }

    #[test]
    fn a_small_screen_holds_fewer_tiles_in_a_row() {
        let (size, spacing, columns) = grid_layout(1280, 270, 6);
        assert!(columns < 6 || size < TILE_MAX, "{size} {columns}");
        assert_eq!(spacing, spacing_for(size));
    }

    #[test]
    fn twenty_tiles_wrap_into_rows_that_fit_the_height() {
        let (size, spacing, columns) = grid_layout(1920, 630, 20);
        let rows = 20usize.div_ceil(columns) as i32;
        assert!(rows > 1 && rows * size + (rows - 1) * spacing <= 630);
        assert!(size >= TILE_MIN, "{size}: still big enough to read");
    }

    #[test]
    fn sixty_tiles_still_fit() {
        for (width, height) in SIZES {
            let (size, ..) = grid_layout(width, height - 450, 60);
            assert!(size >= 1);
            let (across, down) = extent(width, height - 450, 60);
            assert!(
                across <= width - 192 && down <= height - 450,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn more_tiles_never_make_them_bigger() {
        for (width, height) in SIZES {
            let mut last = TILE_MAX;
            for count in 1..=80 {
                let (size, ..) = grid_layout(width, height - 450, count);
                assert!(size <= last, "{count} at {width}: {size} after {last}");
                last = size;
            }
        }
    }

    #[test]
    fn the_layout_never_divides_by_zero_or_vanishes() {
        assert_eq!(grid_layout(1920, 600, 0).0, TILE_MAX);
        for (width, height, count) in [(100, 100, 6), (0, 0, 1), (-5, -5, 3), (1920, 0, 40)] {
            let (size, spacing, columns) = grid_layout(width, height, count);
            assert!(
                size >= 1 && spacing >= 1 && columns >= 1,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn spacing_closes_up_with_the_tiles_but_not_below_the_minimum() {
        assert_eq!(spacing_for(TILE_MAX), TILE_SPACING);
        assert_eq!(spacing_for(TILE_MIN), TILE_MIN / 5);
        assert_eq!(spacing_for(60), TILE_MIN_SPACING);
        assert_eq!(spacing_for(5), 5);
    }

    #[test]
    fn a_tile_moves_a_row_up_or_down_but_not_past_the_ends() {
        // Seven tiles, three to a row: 0 1 2 / 3 4 5 / 6.
        assert_eq!(step_row(4, -1, 3, 7), Some(1));
        assert_eq!(step_row(1, -1, 3, 7), None);
        assert_eq!(step_row(1, 1, 3, 7), Some(4));
        assert_eq!(
            step_row(5, 1, 3, 7),
            Some(6),
            "the last row is short: the last tile"
        );
        assert_eq!(step_row(6, 1, 3, 7), None);
        assert_eq!(step_row(5, 1, 3, 6), None);
        assert_eq!(step_row(0, 1, 7, 7), None, "one row");
        assert_eq!(step_row(0, 1, 3, 0), None);
    }
}
