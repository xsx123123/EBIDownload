use indicatif::ProgressStyle;

pub fn transfer_bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.green} {prefix:<14.bold.cyan} {bar:28.cyan/bright_black} {percent:>3}% {binary_bytes:>9}/{binary_total_bytes:<9} {binary_bytes_per_sec:>10} ETA {eta_precise:>8} {msg:.dim}",
    )
    .expect("valid transfer progress template")
    .progress_chars("█▉▊▋▌▍▎▏░")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

pub fn verify_bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.yellow} {prefix:<14.bold.yellow} {bar:28.green/bright_black} {percent:>3}% {binary_bytes:>9}/{binary_total_bytes:<9} {msg:.dim}",
    )
    .expect("valid verify progress template")
    .progress_chars("█▉▊▋▌▍▎▏░")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

pub fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.green} {prefix:<18.bold.cyan} {msg:.dim}")
        .expect("valid spinner progress template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_styles_are_valid() {
        let _ = transfer_bar_style();
        let _ = verify_bar_style();
        let _ = spinner_style();
    }
}
