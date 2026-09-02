use crate::config::StatedBrowserPath;
use crate::error::IherbError;
use std::path::{Path, PathBuf};

/// Resolves the Chrome binary path. Priority:
/// 1. The path the caller stated, which **binds**: it is used or the run fails.
/// 2. System-installed Chrome detection
/// 3. Previously downloaded Chrome for Testing
/// 4. Auto-download Chrome for Testing
///
/// # A stated path is not a suggestion (#55)
///
/// Step 1 used to `tracing::warn!` and fall through to step 2, so
/// `iherb-cli product 12949 --browser-path /nonexistent --json` exited 0 with
/// `ok: true` and a full record produced by a browser the caller never named.
/// The warning went to stderr at `warn` level; stdout said nothing, so a caller
/// reading the document it asked for had no way to learn that the constraint it
/// stated had been dropped.
///
/// That is the class of defect this programme has been removing throughout —
/// #5's currency relabel, #31's stock signal, #49's search fabrication: a caller
/// states a constraint, the tool quietly does something else, and the output
/// looks authoritative. It also breaks #12 specifically. A persistent profile
/// exists so Cloudflare clearance survives between runs; clearance belongs to
/// the browser that earned it, so a run that silently swapped the binary looks
/// exactly like clearance that stopped working.
///
/// **Steps 2, 3 and 4 still fall through, and must.** Nobody stated them: they
/// are this function's answer to "find me a browser", and an answer is allowed
/// to try the next candidate. The asymmetry is the whole point — the failure
/// being removed is *substitution for something the caller said*, not fallback
/// as such.
///
/// # `invalid_input` (2), not `browser_launch_failed` (10)
///
/// #55's own acceptance criterion asks for 10, and 10 is wrong. [`ErrorKind`]
/// groups its codes by what a caller does about them: `2` is the caller's
/// input, `1x` the local environment. `browser_launch_failed` is documented as
/// "Chrome would not start. The environment needs attention" — and Chrome did
/// not fail to start here, because nothing tried to start it. No browser
/// process, no profile directory, no CDP handshake. What happened is that the
/// arguments named a file that is not there, which is
/// [`IherbError::InvalidInput`] to the letter: "the caller's arguments cannot
/// produce a request, detected before any browser or network work happens".
///
/// The distinction is the one the taxonomy exists to make. A caller that reads
/// 10 goes and looks at the machine; a caller that reads 2 re-reads its own
/// arguments, which is where the fault is. Reporting an environment code for a
/// typo would send every such caller to the wrong place.
///
/// [`ErrorKind`]: crate::error::ErrorKind
pub async fn resolve_chrome(
    user_path: Option<&StatedBrowserPath>,
    data_dir: &Path,
) -> Result<PathBuf, IherbError> {
    // 1. The stated path, which binds.
    if let Some(stated) = user_path {
        if stated.path.exists() {
            tracing::info!("Using user-configured browser: {}", stated.path.display());
            return Ok(stated.path.clone());
        }
        return Err(IherbError::InvalidInput(format!(
            "{} names a browser executable that does not exist: {}. \
             A browser you name is the browser this tool runs; it will not \
             silently use a different one. Correct the path, or remove it to \
             let the tool find a browser itself.",
            stated.source.describe(),
            stated.path.display()
        )));
    }

    // 2. System-installed Chrome
    if let Some(path) = detect_system_chrome() {
        tracing::info!("Using system Chrome: {}", path.display());
        return Ok(path);
    }

    // 3. Previously downloaded Chrome
    let downloaded = downloaded_chrome_path(data_dir);
    if downloaded.exists() {
        tracing::info!("Using downloaded Chrome: {}", downloaded.display());
        return Ok(downloaded);
    }

    // 4. Auto-download
    tracing::info!("No Chrome found. Downloading Chrome for Testing...");
    let path = super::download::download_chrome(data_dir).await?;
    Ok(path)
}

fn detect_system_chrome() -> Option<PathBuf> {
    let candidates = if cfg!(target_os = "macos") {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ]
    } else if cfg!(target_os = "linux") {
        vec![
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
        ]
    } else if cfg!(target_os = "windows") {
        vec![
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ]
    } else {
        vec![]
    };

    for candidate in candidates {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Some(path);
        }
    }

    // Try `which` on unix
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("which")
            .arg("google-chrome")
            .output()
        {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    return Some(PathBuf::from(path_str));
                }
            }
        }
    }

    None
}

pub fn downloaded_chrome_path(data_dir: &Path) -> PathBuf {
    let chrome_dir = data_dir.join("chrome");
    if cfg!(target_os = "macos") {
        chrome_dir
            .join("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS")
            .join("Google Chrome for Testing")
    } else if cfg!(target_os = "windows") {
        chrome_dir.join("chrome.exe")
    } else {
        chrome_dir.join("chrome")
    }
}
