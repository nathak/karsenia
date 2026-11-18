# App icon

`icon-source.webp` is the picture the icon was made from (a tenrec, the spiny animal of Madagascar, happily biting a small code screen, on a dark
rounded square). The others are made from it: the square cut out with its corners transparent (the picture's box is 2% wider than high,
so it is stretched to a square), then scaled.

- `icon.png` 1024 px, `icon-256.png`, `icon-128.png`, `icon-64.png`, `icon-32.png`
- `icon.ico` (16, 24, 32, 48, 64, 128, 256 px): written into the Windows program file by `build.rs` (what Explorer shows)
- `icon-256.png` is the window's icon, `icon-32.png` is built into the tray code, `icon-128.png` is the sidebar's mark, and `ui/favicon.png`
  (64 px) is the web page's

To change the icon, replace these files (same names and sizes). The TENREC wordmark logo is not made yet.
