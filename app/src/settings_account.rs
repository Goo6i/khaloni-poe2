//! "Account" section for the settings window: POESESSID + account name,
//! the live-search list editor, and the wealth-tracker readout. Pure
//! section fn in the `settings_ui` style; the window wires it into its
//! section list.

use eframe::egui;
use khaloni_poe2_core::trade::parse_search_url;

use crate::config::{Config, LiveSearch};
use crate::wealth::WealthSnapshot;

pub fn section_account(ui: &mut egui::Ui, cfg: &mut Config, wealth: &[WealthSnapshot]) {
    use crate::settings_style as st;
    st::card(ui, "account", |ui| {
        st::row(
            ui,
            "POESESSID",
            Some(
                "Your session cookie is stored locally in your config file and sent \
                 only to pathofexile.com. It unlocks live-search alerts and the \
                 wealth tracker; leave it empty to keep both off.",
            ),
            |ui| {
                // Masked like a password: this cookie is a full login credential.
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.poesessid)
                        .password(true)
                        .desired_width(220.0)
                        .hint_text("session cookie"),
                );
            },
        );
        st::row_last(ui, "Account name", None, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut cfg.account_name)
                    .desired_width(220.0)
                    .hint_text("for the wealth tracker"),
            );
        });
    });

    st::group_heading(ui, "Live searches");
    st::note(ui, "Paste a trade-site search URL; new listings raise an alert.");
    ui.add_space(4.0);
    let mut remove: Option<usize> = None;
    if !cfg.live_searches.is_empty() {
        st::card(ui, "live_searches", |ui| {
            let n = cfg.live_searches.len();
            for i in 0..n {
                let search = &mut cfg.live_searches[i];
                // Same red-hint rule the poll worker applies: a URL that does not
                // parse is silently skipped there, so surface it here instead.
                let bad_url = !search.url.trim().is_empty() && parse_search_url(&search.url).is_none();
                let body = |ui: &mut egui::Ui| {
                    if st::button(ui, "✕").clicked() {
                        remove = Some(i);
                    }
                    let w = (ui.available_width() - 136.0).max(120.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut search.url)
                            .desired_width(w)
                            .hint_text("https://www.pathofexile.com/trade2/search/poe2/…"),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut search.name)
                            .desired_width(120.0)
                            .hint_text("name"),
                    );
                };
                ui.push_id(i, |ui| {
                    if i + 1 == n && !bad_url {
                        st::row_last(ui, "", None, body)
                    } else {
                        st::row(ui, "", None, body)
                    }
                    if bad_url {
                        st::warning(ui, "  not a trade search URL (…/trade2/search/poe2/<league>/<id>)");
                    }
                });
            }
        });
    }
    if let Some(i) = remove {
        cfg.live_searches.remove(i);
    }
    if st::button(ui, "+ Add search").clicked() {
        cfg.live_searches.push(LiveSearch {
            name: String::new(),
            url: String::new(),
        });
    }

    st::group_heading(ui, "Wealth");
    match wealth.last() {
        None => {
            st::note(
                ui,
                "No snapshots yet — the tracker records your stash value \
                 every 30 minutes once the cookie and account name are set.",
            );
        }
        Some(latest) => {
            st::card(ui, "wealth", |ui| {
                ui.add_space(8.0);
                ui.strong(format!("{:.1} ex", latest.total_ex));
                egui::Grid::new("wealth_history")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        // Newest first, capped at 10 rows: a readout, not a chart.
                        for snap in wealth.iter().rev().take(10) {
                            ui.weak(ymd_hm_utc(snap.at_epoch_s));
                            ui.label(format!("{:.1} ex", snap.total_ex));
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
            });
        }
    }
}

/// Epoch seconds -> "YYYY-MM-DD HH:MM" in UTC. chrono is deliberately not a
/// dependency (matching settings_ui::hms_now), so the civil date comes from
/// Howard Hinnant's days-from-epoch algorithm.
fn ymd_hm_utc(epoch_s: u64) -> String {
    let (y, m, d) = civil_from_days((epoch_s / 86400) as i64);
    let s = epoch_s % 86400;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", s / 3600, (s % 3600) / 60)
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::ymd_hm_utc;

    #[test]
    fn formats_known_instants() {
        assert_eq!(ymd_hm_utc(0), "1970-01-01 00:00");
        // 2026-07-30 12:34 UTC.
        assert_eq!(ymd_hm_utc(1_785_414_840), "2026-07-30 12:34");
    }
}
