//! Locates libpython and works around distributions that ship the runtime
//! library without the `-dev` symlink the linker needs.
//!
//! Fedora and friends install `libpython3.X.so.1.0` in the runtime package but
//! only create the `libpython3.X.so` link in the `-devel` package. pyo3's own
//! build script emits `-lpython3.X`, which then fails to link. When that
//! happens we create the missing symlink inside OUT_DIR (no root needed) and
//! point the linker at it.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    println!("cargo:rerun-if-env-changed=JF_PYTHON_LIB_DIR");

    let python = std::env::var("PYO3_PYTHON").unwrap_or_else(|_| "python3".to_owned());

    // Ask the interpreter where its library lives and what it is called.
    let script = "import sysconfig; \
                  print(sysconfig.get_config_var('LIBDIR') or ''); \
                  print(sysconfig.get_config_var('LDLIBRARY') or ''); \
                  print(sysconfig.get_config_var('INSTSONAME') or '')";

    let out = match Command::new(&python).arg("-c").arg(script).output() {
        Ok(o) if o.status.success() => o,
        Ok(_) => return,
        Err(_) => return,
    };

    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = text.lines();
    let (Some(libdir), Some(ldlibrary)) = (lines.next(), lines.next()) else { return };
    let soname = lines.next().unwrap_or("").trim();

    let libdir = PathBuf::from(libdir.trim());
    let ldlibrary = ldlibrary.trim();
    if libdir.is_empty() || ldlibrary.is_empty() {
        return;
    }

    // An explicit override wins, for unusual layouts.
    let override_dir = std::env::var("JF_PYTHON_LIB_DIR").ok().map(PathBuf::from);
    if let Some(dir) = override_dir {
        println!("cargo:rustc-link-search=native={}", dir.display());
        return;
    }

    let target = libdir.join(ldlibrary);
    if target.exists() {
        // Development symlink already present: nothing to do.
        return;
    }

    // Find the versioned real library, e.g. libpython3.14.so.1.0
    let stem = ldlibrary.strip_suffix(".so").unwrap_or(ldlibrary);
    let mut found = None;
    if let Ok(entries) = std::fs::read_dir(&libdir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(stem) && name.contains(".so.") {
                found = Some(entry.path());
                break;
            }
        }
    }

    let Some(real) = found else {
        println!("cargo:warning=no libpython found for {python}; pyo3 may fail to link");
        return;
    };

    // Create the symlink in OUT_DIR so no system path is touched.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let link = out_dir.join(ldlibrary);
    if !link.exists() {
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&real, &link);
        // Windows uses a different mechanism and normally has no such gap.
        let _ = &real;
    }

    if link.exists() {
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib={}", ldlibrary.trim_start_matches("lib").trim_end_matches(".so"));
        if !soname.is_empty() {
            let _ = Path::new(soname);
        }
    }
}