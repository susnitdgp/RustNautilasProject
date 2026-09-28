// Keep the process alive through the confirmed 23:15 square-off bar while
// retaining a safety window before the normal 23:30 MCX close.
pub const EXIT_BUFFER_SECONDS: u64 = 10 * 60;
