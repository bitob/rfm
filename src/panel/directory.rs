use std::{
    fs::read_dir,
    os::unix::prelude::MetadataExt,
    slice::{Iter, IterMut},
    time::SystemTime,
};

use crossterm::style::{ContentStyle, StyledContent};
use unix_mode::is_allowed;

use crate::util::file_size_str;

use super::*;
/// An element of a directory.
///
/// Shorthand for saving a path together whith what we want to display.
/// E.g. a file with path `/home/user/something.txt` should only be
/// displayed as `something.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirElem {
    /// Name of the element.
    name: String,

    /// Lowercase name of the element.
    ///
    /// Is saved to save some computation time (and instead increase memory usage).
    lowercase: String,

    /// Full (canonicalized) path of the element
    path: PathBuf,

    /// Weather or not the file is an executable
    is_executable: bool,

    /// String to display either file-size or number of elements in directory
    suffix: String,

    /// True if element is a hidden file or directory.
    is_hidden: bool,

    /// True if the element is marked.
    ///
    /// Users can mark a selected item to perform operations on them.
    is_marked: bool,
}

impl DirElem {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn name_lowercase(&self) -> &String {
        &self.lowercase
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_hidden(&self) -> bool {
        self.is_hidden
    }

    pub fn is_marked(&self) -> bool {
        self.is_marked
    }

    pub fn unmark(&mut self) {
        self.is_marked = false;
    }

    pub fn print_styled(&self, selected: bool, max_len: u16) -> PrintStyledContent<String> {
        let name_len = usize::from(max_len)
            .saturating_sub(self.suffix.len())
            .saturating_sub(4);
        let mut name = format!("{:name_len$}", self.name);
        // We have to truncate the name
        if name.len() > name_len {
            // FIX: If name_len does not lie on a char boundary,
            // the truncate function will panic
            if name.is_char_boundary(name_len) {
                name.truncate(name_len);
            } else {
                // This is stupidly inefficient, but cannot panic.
                while name.len() > name_len {
                    name.pop();
                }
            }
            name.pop();
            name.push('~');
        }
        // let name = self.name.with_exact_width(name_len);

        let string = format!(" {name} {} ", self.suffix);

        let mut style = ContentStyle::new();
        if self.path.is_dir() {
            style = style.dark_green().bold();
        } else if self.is_executable {
            style = style.green().bold();
        } else {
            style = style.grey();
        }
        if self.is_marked {
            style = style.dark_yellow();
        }
        if selected {
            style = style.negative().bold();
        }
        PrintStyledContent(StyledContent::new(style, string))
    }

    pub fn into_parts(self) -> (String, PathBuf) {
        (self.name, self.path)
    }
}

impl<P: AsRef<Path>> From<P> for DirElem {
    fn from(path: P) -> Self {
        let name = path
            .as_ref()
            .file_name()
            .and_then(|p| p.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let lowercase = name.to_lowercase();

        let is_hidden = name.starts_with('.') || name.starts_with("__") || name.ends_with(".swp");

        // Always use an absolute path here
        let path: PathBuf = canonicalize(path.as_ref()).unwrap_or_else(|_| path.as_ref().into());

        let (mode, size) = path
            .metadata()
            .map(|m| (m.permissions().mode(), m.size()))
            .unwrap_or_default();

        let is_executable = is_allowed(unix_mode::Accessor::User, unix_mode::Access::Execute, mode)
            | is_allowed(unix_mode::Accessor::Group, unix_mode::Access::Execute, mode)
            | is_allowed(unix_mode::Accessor::Other, unix_mode::Access::Execute, mode);

        let suffix = if path.is_dir() {
            read_dir(&path)
                .map(|res| res.into_iter().count().to_string())
                .unwrap_or_default()
        } else {
            file_size_str(size)
        };

        DirElem {
            name,
            lowercase,
            path,
            is_hidden,
            suffix,
            is_executable,
            is_marked: false,
        }
    }
}

impl AsRef<DirElem> for DirElem {
    fn as_ref(&self) -> &DirElem {
        self
    }
}

impl PartialOrd for DirElem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.path.is_dir() {
            if other.path.is_dir() {
                return self
                    .name()
                    .to_lowercase()
                    .partial_cmp(&other.name().to_lowercase());
            } else {
                Some(Ordering::Less)
            }
        } else if other.path.is_dir() {
            Some(Ordering::Greater)
        } else {
            return self
                .name()
                .to_lowercase()
                .partial_cmp(&other.name().to_lowercase());
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirPanel {
    /// Elements of the directory
    elements: Vec<DirElem>,

    /// Non-hidden elements (saved by their index)
    ///
    /// NOTE: The elements vector *must not change* over the lifetime of the panel.
    /// Otherwise the indizes in this vector would be invalid
    non_hidden: Vec<usize>,

    /// Selected element
    selected_idx: usize,

    /// Index in the `non_hidden` vector that is our current selection
    non_hidden_idx: usize,

    /// Path of the directory that the panel is based on
    path: PathBuf,

    /// Last modification time.
    modified: SystemTime,

    /// Weather or not the panel is still loading some data
    loading: bool,

    /// Weather or not to show hidden files
    show_hidden: bool,

    /// Hash of the elements
    hash: u64,
}

impl Draw for DirPanel {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        // Calculate page-scroll
        let scroll: usize = {
            // if selected should be in the middle all the time:
            // bot = min(max-items, selected + height / 2)
            // scroll = min(0, bot - (height + 1))
            let h = (height.saturating_add(1)) as usize / 2;
            let bot = if self.show_hidden {
                self.elements.len().min(self.selected_idx.saturating_add(h))
            } else {
                self.non_hidden
                    .len()
                    .min(self.non_hidden_idx.saturating_add(h))
                    .saturating_add(1)
            };
            bot.saturating_sub(height as usize)
        };

        // Then print new buffer
        let mut y_offset = 0_u16;
        // Write "height" items to the screen
        for (idx, entry) in self
            .elements
            .iter()
            .enumerate()
            .skip(scroll)
            .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
            .take(height as usize)
        {
            let y = y_range.start + y_offset;
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("│".dark_green().bold()),
                entry.print_styled(self.selected_idx == idx, width),
                // cursor::MoveTo(x_range.start.saturating_add(1), y),
                // PrintStyledContent("asdf".dark_red().bold()),
            )?;
            y_offset += 1;
        }

        for y in (y_range.start + y_offset)..y_range.end {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("│".dark_green().bold()),
            )?;
            for x in x_range.start + 1..x_range.end {
                queue!(stdout, cursor::MoveTo(x, y), Print(" "),)?;
            }
        }

        // Check if we are loading or not
        if self.loading {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start + 2, y_range.start + 1),
                PrintStyledContent("Loading...".dark_green().bold().italic()),
                cursor::MoveTo(x_range.start + 2, y_range.start + 2),
                PrintStyledContent(
                    format!("{}", self.path.display())
                        .with_exact_width(width.saturating_sub(2) as usize)
                        .dark_green()
                        .italic()
                ),
            )?;
        } else if self.elements.is_empty() {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start + 1, y_range.start),
                PrintStyledContent("(empty)".dark_grey().italic()),
            )?;
        }
        Ok(())
    }
}

impl PanelContent for DirPanel {
    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn content_hash(&self) -> u64 {
        self.hash
    }

    fn modified(&self) -> SystemTime {
        self.modified
    }

    fn update_content(&mut self, mut content: Self) {
        // Keep "hidden" state
        content.show_hidden = self.show_hidden;
        // If the content is for the same directory
        if content.path == self.path {
            // Set the selection accordingly
            if let Some(path) = self.selected_path() {
                content.select_path(path);
            }
        }
        *self = content;
    }
}

impl BasePanel for DirPanel {
    fn empty() -> Self {
        DirPanel::empty()
    }

    fn loading(path: PathBuf) -> Self {
        DirPanel::loading(path)
    }
}

impl DirPanel {
    pub fn new(elements: Vec<DirElem>, path: PathBuf) -> Self {
        let non_hidden = elements
            .iter()
            .enumerate()
            .filter(|(_, elem)| !elem.is_hidden)
            .map(|(idx, _)| idx)
            .collect::<Vec<usize>>();

        let selected = *non_hidden.first().unwrap_or(&0);
        let hash = hash_elements(&elements);

        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or_else(SystemTime::now);

        DirPanel {
            elements,
            non_hidden,
            selected_idx: selected,
            non_hidden_idx: 0,
            path,
            modified,
            loading: false,
            show_hidden: false,
            hash,
        }
    }

    pub fn elements(&self) -> Iter<DirElem> {
        self.elements.iter()
    }

    pub fn elements_mut(&mut self) -> IterMut<DirElem> {
        self.elements.iter_mut()
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    pub fn mark_selected_item(&mut self) {
        if let Some(elem) = self.elements.get_mut(self.selected_idx) {
            elem.is_marked = !elem.is_marked;
        }
    }

    /// Changes the selection to the given path.
    ///
    /// If the path is not found, the selection remains unchanged.
    pub fn select_path(&mut self, selection: &Path) {
        // Do nothing if the path is already selected
        if self.selected_path() == Some(selection) {
            return;
        }
        self.selected_idx = self
            .elements
            .iter()
            .enumerate()
            .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
            .find(|(_, elem)| elem.path() == selection)
            .map(|(idx, _)| idx)
            .unwrap_or(self.selected_idx);
        if !self.show_hidden {
            self.set_non_hidden_idx();
        }
    }

    /// Sets non-hidden-idx to the value closest to selection
    fn set_non_hidden_idx(&mut self) {
        for (idx, elem_idx) in self.non_hidden.iter().enumerate() {
            self.non_hidden_idx = idx;
            if *elem_idx >= self.selected_idx {
                break;
            }
        }
    }

    pub fn set_hidden(&mut self, show_hidden: bool) {
        if self.show_hidden == show_hidden {
            // Nothing to do
            return;
        }
        if self.show_hidden && !show_hidden {
            // Currently we show hidden files, but we should stop that
            // -> non-hidden-idx needs to be updated to the value closest to selection
            self.set_non_hidden_idx();
            // Update selection accordingly for the next time we toggle hidden files
            self.selected_idx = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        // Save value and change selection accordingly
        self.show_hidden = show_hidden;
    }

    pub fn loading(path: PathBuf) -> Self {
        DirPanel {
            elements: Vec::new(),
            non_hidden: Vec::new(),
            selected_idx: 0,
            non_hidden_idx: 0,
            path,
            modified: SystemTime::now(),
            loading: true,
            show_hidden: false,
            hash: 0,
        }
    }

    /// Creates an empty dir-panel.
    ///
    /// Note: The path of this panel is not a valid path!
    pub fn empty() -> Self {
        DirPanel {
            elements: Vec::new(),
            non_hidden: Vec::new(),
            selected_idx: 0,
            non_hidden_idx: 0,
            modified: SystemTime::now(),
            path: "path-of-empty-panel".into(),
            loading: false,
            show_hidden: false,
            hash: 0,
        }
    }

    /// Move the selection "up" if possible.
    ///
    /// Returns true if the panel has changed and
    /// requires a redraw.
    pub fn up(&mut self, step: usize) -> bool {
        if self.show_hidden {
            if self.selected_idx == 0 {
                return false;
            }
            self.selected_idx = self.selected_idx.saturating_sub(step);
        } else {
            if self.non_hidden_idx == 0 {
                return false;
            }
            self.non_hidden_idx = self.non_hidden_idx.saturating_sub(step);
            self.selected_idx = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        true
    }

    /// Move the selection "down" if possible.
    ///
    /// Returns true if the panel has changed and
    /// requires a redraw.
    pub fn down(&mut self, step: usize) -> bool {
        if self.show_hidden {
            // If we are already at the end, do nothing and return
            if self.selected_idx.saturating_add(1) == self.elements.len() {
                return false;
            }
            // If step is too big, just jump to the end
            if self.selected_idx.saturating_add(step) >= self.elements.len() {
                // selected = len(elements) - 1
                self.selected_idx = self.elements.len().saturating_sub(1);
            } else {
                // Otherwise just increase by step
                self.selected_idx = self.selected_idx.saturating_add(step);
            }
        } else {
            // If we are already at the end, do nothing and return
            if self.non_hidden_idx.saturating_add(1) == self.non_hidden.len() {
                return false;
            }
            if self.non_hidden_idx.saturating_add(step) >= self.non_hidden.len() {
                // idx = len(non_hidden) - 1
                self.non_hidden_idx = self.non_hidden.len().saturating_sub(1);
            } else {
                self.non_hidden_idx = self.non_hidden_idx.saturating_add(step);
            }
            self.selected_idx = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        true
    }

    /// Returns the selcted path of the panel.
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|elem| elem.path())
    }

    /// Returns either the selected-idx or non-hidden-idx,
    /// depending on weather or not we display hidden files.
    pub fn index(&self) -> usize {
        if self.show_hidden {
            self.selected_idx
        } else {
            self.non_hidden_idx
        }
    }

    /// Returns the selcted path of the panel as an owned `PathBuf`.
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected_path_owned(&self) -> Option<PathBuf> {
        self.selected_path().map(|p| p.to_path_buf())
    }

    /// Returns a reference to the selected [`DirElem`].
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected(&self) -> Option<&DirElem> {
        self.elements.get(self.selected_idx)
    }

    /// Returns the selected index (starting at 1) and the total number of items.
    pub fn index_vs_total(&self) -> (usize, usize) {
        if self.show_hidden {
            (self.selected_idx.saturating_add(1), self.elements.len())
        } else {
            (self.non_hidden_idx.saturating_add(1), self.non_hidden.len())
        }
    }
}
