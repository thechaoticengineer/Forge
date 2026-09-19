//! Grammar of the `Business tests:` registry lines (D16): separators, notes,
//! backticks and the validation reasons for malformed entries.

use crate::features::{self, Feature};
use std::fs;
use std::path::{Path, PathBuf};

struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("forge-registry-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Project(path)
    }

    fn file(&self, rel: &str) {
        let target = self.0.join(rel);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, "x\n").unwrap();
    }

    fn feature(&self, slug: &str, milestones: &str) -> Feature {
        let dir = self.0.join("docs/features").join(slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("README.md"), format!("# {slug}\n")).unwrap();
        fs::write(dir.join("scenarios.md"), "## S1: One\n\n## S2: Two\n").unwrap();
        fs::write(dir.join("decisions.md"), "## D1: A decision\n").unwrap();
        fs::write(dir.join("milestones.md"), milestones).unwrap();
        features::discover(&self.0).into_iter().find(|f| f.slug == slug).unwrap()
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn milestone(line: &str) -> String {
    format!("## M1: First\n\nCovers: S1, S2\n{line}\n")
}

#[test]
fn notes_may_contain_commas_and_the_word_and() {
    let project = Project::new("notes");
    project.file("src/a.rs");
    project.file("tests/b.mjs");
    let f = project.feature("f", &milestone("Business tests: src/a.rs (S1, S2 and more) and tests/b.mjs (a, b)"));
    assert!(f.reasons.is_empty(), "{:?}", f.reasons);
    assert_eq!(f.milestones[0].business_tests, ["src/a.rs", "tests/b.mjs"]);
    assert_eq!(f.milestones[0].covers, ["S1", "S2"]);
}

#[test]
fn backticked_paths_split_on_commas_and_and() {
    let project = Project::new("ticks");
    for p in ["a.rs", "b.rs", "c.rs"] {
        project.file(p);
    }
    let f = project.feature("f", &milestone("Business tests: `a.rs`, `b.rs` and `c.rs`"));
    assert!(f.reasons.is_empty(), "{:?}", f.reasons);
    assert_eq!(f.milestones[0].business_tests, ["a.rs", "b.rs", "c.rs"]);
}

#[test]
fn a_path_containing_and_is_not_split() {
    let project = Project::new("word");
    project.file("tests/band.rs");
    project.file("tests/and.rs");
    let f = project.feature("f", &milestone("Business tests: tests/band.rs and tests/and.rs"));
    assert!(f.reasons.is_empty(), "{:?}", f.reasons);
    assert_eq!(f.milestones[0].business_tests, ["tests/band.rs", "tests/and.rs"]);
}

#[test]
fn duplicate_entries_register_once() {
    let project = Project::new("dupes");
    project.file("a.rs");
    let f = project.feature("f", &milestone("Business tests: a.rs, `a.rs` (again)"));
    assert!(f.reasons.is_empty(), "{:?}", f.reasons);
    assert_eq!(f.milestones[0].business_tests, ["a.rs"]);
}

#[test]
fn a_missing_file_is_reported_by_path_and_valid_entries_stay_registered() {
    let project = Project::new("missing");
    project.file("a.rs");
    let f = project.feature("f", &milestone("Business tests: a.rs and tests/gone.rs (S1)"));
    assert_eq!(f.reasons, ["business test file not found in M1: tests/gone.rs"]);
    assert_eq!(f.milestones[0].business_tests, ["a.rs"]);
    assert_eq!(features::registered_business_tests(project.path()), ["a.rs"]);
}

#[test]
fn an_entry_with_spaces_or_an_unsafe_path_is_malformed() {
    let project = Project::new("malformed");
    project.file("a.rs");
    for entry in ["two words", "../escape.rs", "/etc/passwd", "(only a note)", ""] {
        let f = project.feature("f", &milestone(&format!("Business tests: a.rs, {entry}")));
        assert_eq!(
            f.reasons,
            [format!("malformed Business tests entry in M1: {entry}")],
            "entry {entry:?}"
        );
        assert_eq!(f.milestones[0].business_tests, ["a.rs"]);
    }
}

#[test]
fn a_repeated_line_is_an_error() {
    let project = Project::new("repeat");
    project.file("a.rs");
    let f = project.feature("f", &milestone("Business tests: a.rs\nBusiness tests: a.rs"));
    assert_eq!(f.reasons, ["repeated Business tests line in M1"]);
}

#[test]
fn a_line_outside_a_milestone_is_an_error() {
    let project = Project::new("outside");
    project.file("a.rs");
    let f = project.feature("f", "Business tests: a.rs\n\n## M1: First\n\nCovers: S1\n");
    assert_eq!(f.reasons, ["Business tests line outside a milestone: a.rs"]);
    assert!(f.milestones[0].business_tests.is_empty());
}

#[test]
fn each_milestone_may_carry_its_own_line() {
    let project = Project::new("two");
    project.file("a.rs");
    project.file("b.rs");
    let f = project.feature(
        "f",
        "## M1: First\nCovers: S1\nBusiness tests: a.rs\n\n## M2: Second\nCovers: none yet\nBusiness tests: b.rs\n",
    );
    assert!(f.reasons.is_empty(), "{:?}", f.reasons);
    assert_eq!(f.milestones[0].business_tests, ["a.rs"]);
    assert_eq!(f.milestones[1].business_tests, ["b.rs"]);
    assert!(f.milestones[1].covers.is_empty());
}

#[test]
fn the_global_registry_is_sorted_and_deduplicated_across_features() {
    let project = Project::new("global");
    for p in ["z.rs", "a.rs", "m.rs"] {
        project.file(p);
    }
    project.feature("one", &milestone("Business tests: z.rs and a.rs"));
    project.feature("two", &milestone("Business tests: a.rs, m.rs"));
    assert_eq!(features::registered_business_tests(project.path()), ["a.rs", "m.rs", "z.rs"]);
}

#[test]
fn every_repository_feature_is_valid_and_registers_only_existing_files() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let found = features::discover(repo);
    assert!(!found.is_empty(), "the repository must have feature specs");
    for feature in &found {
        assert_eq!(feature.status(), "valid", "{}: {:?}", feature.slug, feature.reasons);
        for milestone in &feature.milestones {
            for file in &milestone.business_tests {
                assert!(repo.join(file).is_file(), "{} {} registers {file}, which is not a file", feature.slug, milestone.id);
            }
        }
    }
}

#[test]
fn declared_entries_include_missing_files_and_skip_malformed_ones() {
    let text = "## M1: One\n\nBusiness tests: `tests/a.rs` (S1, and S2), tests/gone.rs and some words\n\n\
        ```\nBusiness tests: tests/fenced.rs\n```\n\n## M2: Two\n\nBusiness tests: tests/a.rs and tests/b.mjs (S3)\n";
    assert_eq!(features::declared_business_tests(text), ["tests/a.rs", "tests/gone.rs", "tests/b.mjs"]);
    assert!(features::declared_business_tests("## M1: One\n\nStatus: planned\n").is_empty());
}
