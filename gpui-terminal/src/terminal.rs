use crate::event::GpuiEventProxy;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use parking_lot::Mutex;
use std::sync::Arc;

struct TermDimensions {
    columns: usize,
    screen_lines: usize,
}

impl TermDimensions {
    fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
        }
    }
}

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {

        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }

    fn last_column(&self) -> alacritty_terminal::index::Column {
        alacritty_terminal::index::Column(self.columns.saturating_sub(1))
    }
}

pub struct TerminalState {

    term: Arc<Mutex<Term<GpuiEventProxy>>>,

    parser: Processor,

    cols: usize,

    rows: usize,
}

impl TerminalState {

    pub fn new(cols: usize, rows: usize, event_proxy: GpuiEventProxy) -> Self {

        let config = Config::default();

        let dimensions = TermDimensions::new(cols, rows);

        let term = Term::new(config, &dimensions, event_proxy);

        let parser = Processor::new();

        Self {
            term: Arc::new(Mutex::new(term)),
            parser,
            cols,
            rows,
        }
    }

    pub fn process_bytes(&mut self, bytes: &[u8]) {
        let mut term = self.term.lock();

        self.parser.advance(&mut *term, bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;

        let mut term = self.term.lock();

        let dimensions = TermDimensions::new(cols, rows);

        term.resize(dimensions);
    }

    pub fn mode(&self) -> TermMode {
        let term = self.term.lock();
        *term.mode()
    }

    pub fn with_term<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Term<GpuiEventProxy>) -> R,
    {
        let term = self.term.lock();
        f(&term)
    }

    pub fn with_term_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Term<GpuiEventProxy>) -> R,
    {
        let mut term = self.term.lock();
        f(&mut term)
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn term_arc(&self) -> Arc<Mutex<Term<GpuiEventProxy>>> {
        Arc::clone(&self.term)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn test_terminal_creation() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let terminal = TerminalState::new(80, 24, event_proxy);

        assert_eq!(terminal.cols(), 80);
        assert_eq!(terminal.rows(), 24);
    }

    #[test]
    fn test_process_bytes() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let mut terminal = TerminalState::new(80, 24, event_proxy);

        terminal.process_bytes(b"Hello, world!");

        terminal.with_term(|term| {
            let grid = term.grid();

            assert!(grid.columns() == 80);
        });
    }

    #[test]
    fn test_resize() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let mut terminal = TerminalState::new(80, 24, event_proxy);

        terminal.resize(120, 30);

        assert_eq!(terminal.cols(), 120);
        assert_eq!(terminal.rows(), 30);

        terminal.with_term(|term| {
            let grid = term.grid();
            assert_eq!(grid.columns(), 120);
            assert_eq!(grid.screen_lines(), 30);
        });
    }

    #[test]
    fn test_mode() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let terminal = TerminalState::new(80, 24, event_proxy);

        let mode = terminal.mode();

        let _bits = mode.bits();
    }

    #[test]
    fn test_with_term() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let terminal = TerminalState::new(80, 24, event_proxy);

        let cols = terminal.with_term(|term| term.grid().columns());
        assert_eq!(cols, 80);
    }

    #[test]
    fn test_with_term_mut() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let terminal = TerminalState::new(80, 24, event_proxy);

        terminal.with_term_mut(|term| {

            let _grid = term.grid_mut();
        });
    }

    #[test]
    fn test_term_arc() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let terminal = TerminalState::new(80, 24, event_proxy);

        let arc1 = terminal.term_arc();
        let arc2 = terminal.term_arc();

        assert!(Arc::ptr_eq(&arc1, &arc2));
    }

    #[test]
    fn test_scroll_direction() {
        let (tx, _rx) = channel();
        let event_proxy = GpuiEventProxy::new(tx);
        let mut terminal = TerminalState::new(80, 24, event_proxy);
        for i in 0..100 {
            terminal.process_bytes(format!("Line {}\r\n", i).as_bytes());
        }
        terminal.with_term_mut(|term| {
            let initial = term.grid().display_offset();
            assert_eq!(initial, 0);
            term.scroll_display(alacritty_terminal::grid::Scroll::Delta(5));
            assert_eq!(term.grid().display_offset(), 5);
            term.scroll_display(alacritty_terminal::grid::Scroll::Delta(-5));
            assert_eq!(term.grid().display_offset(), 0);
        });
    }
}
