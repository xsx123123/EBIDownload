use indicatif::ProgressStyle;

pub fn transfer_bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.green} {prefix:<14.bold.cyan} [{wide_bar:.cyan/blue}] {percent:>3}% {binary_bytes:>8}/{binary_total_bytes:<8} {binary_bytes_per_sec:>10} ETA {eta_precise:>8} {msg}",
    )
    .expect("valid transfer progress template")
    .progress_chars("=>-")
}

pub fn verify_bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.yellow} {prefix:<14.bold.yellow} [{wide_bar:.green/white}] {percent:>3}% {binary_bytes:>8}/{binary_total_bytes:<8} {binary_bytes_per_sec:>10} {msg}",
    )
    .expect("valid verify progress template")
    .progress_chars("=>-")
}

pub fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{prefix:<18.bold.dim} {spinner:.green} {msg}")
        .expect("valid spinner progress template")
        .tick_chars("-\\|/")
}
