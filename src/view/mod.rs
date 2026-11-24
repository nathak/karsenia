//! What the desktop app shows, worked out without any UI toolkit: the words (`i18n`), an answer's markdown made safe
//! (`markdown`) and a chat's events turned into rows (`transcript`). Tested here; the Slint code only draws it.
pub mod i18n;
pub mod markdown;
pub mod attach;
pub mod cost;
pub mod debug;
pub mod files;
pub mod git;
pub mod highlight;
pub mod guardfile;
pub mod import;
pub mod notify;
pub mod palette;
pub mod panels;
pub mod project_check;
pub mod sched;
pub mod slash;
pub mod swb;
pub mod team;
pub mod term;
pub mod transcript;
pub mod usage;
pub mod vhdb;
pub mod wt;
