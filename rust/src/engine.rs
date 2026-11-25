use crate::{
    model::{Finding, relative_name},
    reporting::registry,
    workspace::{Workspace, matches_glob},
};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};
#[derive(Default)]
pub struct Result {
    pub findings: Vec<Finding>,
    pub baselined: Vec<Finding>,
    pub ignored: Vec<Finding>,
}
pub fn key(f: &Finding) -> (String, String, String) {
    (f.filename.clone(), f.code.clone(), f.message.clone())
}
pub fn sort(findings: &mut Vec<Finding>) {
    findings.par_sort_unstable();
    findings.dedup();
}
pub fn run(w: &mut Workspace, selected: &[PathBuf], no_baseline: bool) -> Result {
    let selected_path = |p: &Path| selected.is_empty() || selected.iter().any(|s| p.starts_with(s));
    let manifest = w
        .manifest
        .as_ref()
        .map_or("leesh.toml".into(), |p| relative_name(p, &w.root));
    let problem = |message: String| Finding {
        filename: manifest.clone(),
        line: 1,
        column: 0,
        code: "LIT003".into(),
        message,
        target: None,
    };
    let mut problems: Vec<_> = w.configuration_errors().into_iter().map(&problem).collect();
    for (path, error) in &w.parse_failures {
        if selected_path(path) {
            problems.push(Finding {
                filename: relative_name(path, &w.root),
                line: error.line,
                column: error.column,
                code: "LIT001".into(),
                message: format!("file cannot be parsed: {}", error.message),
                target: None,
            });
        }
    }
    // Precompute SCCs once before parallel file checks so workers never block on OnceLock.
    crate::dependencies::precompute_sccs(w);
    let cross_findings: Vec<Vec<Finding>> = w
        .sources
        .par_iter()
        .enumerate()
        .filter(|(_, s)| selected_path(&s.path))
        .map(|(idx, s)| {
            let mut f = Vec::new();
            crate::rules::check_cross_file(idx, s, w, &mut f);
            f.extend(crate::dependencies::check(idx, s, w));
            f
        })
        .collect();
    let total_cap: usize = w
        .sources
        .iter()
        .filter(|s| selected_path(&s.path))
        .map(|s| s.local_findings.len())
        .sum::<usize>()
        + cross_findings.iter().map(Vec::len).sum::<usize>();
    let mut found = Vec::with_capacity(total_cap);
    for s in &mut w.sources {
        if selected_path(&s.path) {
            found.append(&mut s.local_findings);
        }
    }
    for mut chunk in cross_findings {
        found.append(&mut chunk);
    }
    sort(&mut found);
    let mut ignores = Vec::new();
    for ignore in &w.ignores {
        let mut codes = BTreeSet::new();
        for name in &ignore.rules {
            match registry()
                .iter()
                .find(|r| r.code == *name || r.name == *name)
            {
                None => problems.push(problem(format!("ignore names unknown rule {name}"))),
                Some(r) if ["LIT001", "LIT003"].contains(&r.code.as_str()) => {
                    problems.push(problem(format!("{} cannot be ignored", r.code)))
                }
                Some(r) => {
                    codes.insert(r.code.clone());
                }
            }
        }
        ignores.push((ignore, codes));
    }
    let layer_by_filename: HashMap<&str, &str> =
        if ignores.iter().any(|(i, _)| !i.layers.is_empty()) {
            w.sources
                .iter()
                .filter_map(|s| {
                    s.classification
                        .as_ref()
                        .map(|c| (s.filename.as_str(), c.name.as_str()))
                })
                .collect()
        } else {
            HashMap::new()
        };
    let mut used: BTreeMap<usize, usize> = BTreeMap::new();
    let mut kept = Vec::new();
    let mut ignored = Vec::new();
    if ignores.is_empty() {
        kept = found;
    } else {
        for f in found {
            let matched = ignores.iter().position(|(i, codes)| {
                codes.contains(&f.code)
                    && (i.paths.is_empty() || i.paths.iter().any(|p| matches_glob(&f.filename, p)))
                    && (i.layers.is_empty()
                        || layer_by_filename
                            .get(f.filename.as_str())
                            .is_some_and(|layer| w.policy.is_a(layer, &i.layers)))
            });
            if let Some(i) = matched {
                *used.entry(i).or_default() += 1;
                ignored.push(f);
            } else {
                kept.push(f);
            }
        }
    }
    let describe = |i: &crate::workspace::Ignore| {
        let mut scope = i.paths.clone();
        scope.extend(i.layers.iter().cloned());
        if scope.is_empty() {
            i.rules.join(", ")
        } else {
            format!("{} in {}", i.rules.join(", "), scope.join(", "))
        }
    };
    if selected.is_empty() {
        for (index, (i, codes)) in ignores.iter().enumerate() {
            let count = used.get(&index).copied().unwrap_or(0);
            if count == 0 && !codes.is_empty() {
                problems.push(problem(format!(
                    "ignore suppresses nothing: {}",
                    describe(i)
                )));
            } else if let Some(max_count) = i.max_count
                && count > max_count
            {
                problems.push(problem(format!(
                    "ignore max_count exceeded for {}: suppressed {count}, max_count is {max_count}",
                    describe(i)
                )));
            }
        }
    }
    for i in &w.ignores {
        if let Some(review_by) = &i.review_by
            && review_by.as_str() < w.today.as_str()
        {
            problems.push(problem(format!(
                "ignore review_by {review_by} has expired: {}",
                describe(i)
            )));
        }
    }
    let mut baselined = Vec::new();
    if let Some(baseline_path) = w.baseline_path.as_ref().filter(|_| !no_baseline) {
        let mut remaining: BTreeMap<_, _> = w.baseline.clone();
        for f in kept {
            let count = remaining.entry(key(&f)).or_default();
            if *count > 0 {
                *count -= 1;
                baselined.push(f);
            } else {
                problems.push(f);
            }
        }
        let stale: usize = remaining
            .iter()
            .filter(|((filename, _, _), _)| selected_path(&w.root.join(filename)))
            .map(|(_, n)| n)
            .sum();
        if stale > 0 {
            problems.push(Finding {
                filename: relative_name(baseline_path, &w.root),
                line: 1,
                column: 0,
                code: "LIT003".into(),
                message: format!(
                    "{stale} baseline {} no longer {} a finding; run `leesh baseline --prune` to shrink the baseline",
                    if stale == 1 { "entry" } else { "entries" },
                    if stale == 1 { "matches" } else { "match" }
                ),
                target: None,
            });
        }
    } else if problems.is_empty() {
        // `kept` is already sorted & deduplicated when no problems are merged in.
        return Result {
            findings: kept,
            baselined,
            ignored,
        };
    } else {
        problems.extend(kept);
    }
    sort(&mut problems);
    Result {
        findings: problems,
        baselined,
        ignored,
    }
}
