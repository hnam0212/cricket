//! Matching an app (executable name) to an SMTC media session.
//!
//! SMTC identifies sessions by AppUserModelId, not by process. Win32 apps
//! usually report their executable name (`Spotify.exe`, `chrome.exe`), some
//! report a bare name (`Chrome`), and packaged apps report a package id
//! (`SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify`). There is no exact
//! mapping, so this is a heuristic. The probe prints the raw ids so a
//! mismatch is easy to spot.

use cricket_core::audio::AppId;

/// How well an AppUserModelId matches an app. Lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchRank {
    /// The id is the executable name, with or without `.exe`.
    Exact,
    /// The id contains the executable name without `.exe`.
    Contains,
}

pub fn match_aumid(aumid: &str, app: &AppId) -> Option<MatchRank> {
    let aumid = aumid.trim().to_lowercase();
    let exe = app.as_str();
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    if stem.is_empty() {
        return None;
    }
    if aumid == exe || aumid == stem {
        Some(MatchRank::Exact)
    } else if aumid.contains(stem) {
        Some(MatchRank::Contains)
    } else {
        None
    }
}

/// Index of the best matching id, preferring exact matches and then the
/// earliest entry.
pub fn best_match<S: AsRef<str>>(aumids: &[S], app: &AppId) -> Option<usize> {
    aumids
        .iter()
        .enumerate()
        .filter_map(|(index, aumid)| match_aumid(aumid.as_ref(), app).map(|rank| (rank, index)))
        .min()
        .map(|(_, index)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_name_matches_exactly() {
        let spotify = AppId::new("spotify.exe");
        assert_eq!(match_aumid("Spotify.exe", &spotify), Some(MatchRank::Exact));
        assert_eq!(match_aumid("Spotify", &spotify), Some(MatchRank::Exact));
    }

    #[test]
    fn packaged_app_matches_by_substring() {
        let spotify = AppId::new("spotify.exe");
        assert_eq!(
            match_aumid("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify", &spotify),
            Some(MatchRank::Contains)
        );
    }

    #[test]
    fn unrelated_app_does_not_match() {
        assert_eq!(match_aumid("Chrome", &AppId::new("spotify.exe")), None);
        assert_eq!(match_aumid("Chrome", &AppId::new("")), None);
        assert_eq!(match_aumid("Chrome", &AppId::new(".exe")), None);
    }

    #[test]
    fn best_match_prefers_exact_over_substring() {
        let ids = [
            "Microsoft.ZuneMusic_8wekyb3d8bbwe!chromeish",
            "Chrome",
            "chrome.exe",
        ];
        assert_eq!(best_match(&ids, &AppId::new("chrome.exe")), Some(1));
        assert_eq!(best_match(&ids, &AppId::new("vlc.exe")), None);
    }
}
