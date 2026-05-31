use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SourceClass {
    Paper,
    Github,
    StackOverflow,
    Forum,
    News,
    Government,
    Docs,
    Pdf,
    Product,
    Blog,
    Unknown,
}

impl SourceClass {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceClass::Paper => "paper",
            SourceClass::Github => "github",
            SourceClass::StackOverflow => "stackoverflow",
            SourceClass::Forum => "forum",
            SourceClass::News => "news",
            SourceClass::Government => "government",
            SourceClass::Docs => "docs",
            SourceClass::Pdf => "pdf",
            SourceClass::Product => "product",
            SourceClass::Blog => "blog",
            SourceClass::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CitationStrategy {
    Bibliography,
    Inline,
    Url,
    Snapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtractionProfile {
    pub source_class: SourceClass,
    pub favor_recall: bool,
    pub include_tables: bool,
    pub include_links: bool,
    pub include_images: bool,
    pub fast_extract: bool,
    pub snippet_max_chars: usize,
    pub citation_strategy: CitationStrategy,
}

impl Default for ExtractionProfile {
    fn default() -> Self {
        profile_for(SourceClass::Unknown)
    }
}

pub fn classify_url(url: &Url) -> SourceClass {
    let host = normalize_host(url.host_str().unwrap_or(""));
    let path = url.path().to_ascii_lowercase();

    if path.ends_with(".pdf") {
        return SourceClass::Pdf;
    }
    if matches_host(
        &host,
        &[
            "arxiv.org",
            "doi.org",
            "pubmed.ncbi.nlm.nih.gov",
            "semanticscholar.org",
        ],
    ) {
        return SourceClass::Paper;
    }
    if matches_host(&host, &["github.com", "github.io", "gitlab.com"]) {
        return SourceClass::Github;
    }
    if matches_host(&host, &["stackoverflow.com", "stackexchange.com"]) {
        return SourceClass::StackOverflow;
    }
    if host.ends_with(".gov") || host.ends_with(".mil") {
        return SourceClass::Government;
    }
    if matches_host(
        &host,
        &["docs.rs", "developer.mozilla.org", "readthedocs.io"],
    ) || path.contains("/docs")
        || path.contains("/documentation")
    {
        return SourceClass::Docs;
    }
    if matches_host(
        &host,
        &[
            "nytimes.com",
            "reuters.com",
            "apnews.com",
            "bbc.com",
            "theguardian.com",
        ],
    ) {
        return SourceClass::News;
    }
    if matches_host(
        &host,
        &[
            "medium.com",
            "substack.com",
            "wordpress.com",
            "blogspot.com",
        ],
    ) || path.contains("/blog")
    {
        return SourceClass::Blog;
    }
    if path.contains("/forum") || path.contains("/community") || path.contains("/discuss") {
        return SourceClass::Forum;
    }
    if path.contains("/product") || path.contains("/pricing") {
        return SourceClass::Product;
    }

    SourceClass::Unknown
}

pub fn profile_for_url(url: &Url) -> ExtractionProfile {
    profile_for(classify_url(url))
}

pub fn profile_for(source_class: SourceClass) -> ExtractionProfile {
    match source_class {
        SourceClass::Paper => ExtractionProfile {
            source_class,
            favor_recall: true,
            include_tables: true,
            include_links: true,
            include_images: false,
            fast_extract: false,
            snippet_max_chars: 600,
            citation_strategy: CitationStrategy::Bibliography,
        },
        SourceClass::Github | SourceClass::Docs | SourceClass::Government => ExtractionProfile {
            source_class,
            favor_recall: true,
            include_tables: true,
            include_links: true,
            include_images: false,
            fast_extract: false,
            snippet_max_chars: 420,
            citation_strategy: CitationStrategy::Url,
        },
        SourceClass::StackOverflow | SourceClass::Forum => ExtractionProfile {
            source_class,
            favor_recall: false,
            include_tables: false,
            include_links: true,
            include_images: false,
            fast_extract: true,
            snippet_max_chars: 320,
            citation_strategy: CitationStrategy::Inline,
        },
        SourceClass::News | SourceClass::Blog | SourceClass::Product => ExtractionProfile {
            source_class,
            favor_recall: false,
            include_tables: false,
            include_links: true,
            include_images: false,
            fast_extract: true,
            snippet_max_chars: 360,
            citation_strategy: CitationStrategy::Url,
        },
        SourceClass::Pdf => ExtractionProfile {
            source_class,
            favor_recall: true,
            include_tables: true,
            include_links: false,
            include_images: false,
            fast_extract: false,
            snippet_max_chars: 600,
            citation_strategy: CitationStrategy::Snapshot,
        },
        SourceClass::Unknown => ExtractionProfile {
            source_class,
            favor_recall: false,
            include_tables: false,
            include_links: true,
            include_images: false,
            fast_extract: true,
            snippet_max_chars: 240,
            citation_strategy: CitationStrategy::Url,
        },
    }
}

fn normalize_host(host: &str) -> String {
    host.trim_start_matches("www.").to_ascii_lowercase()
}

fn matches_host(host: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| host == *candidate || host.ends_with(&format!(".{candidate}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(raw: &str) -> SourceClass {
        classify_url(&Url::parse(raw).unwrap())
    }

    #[test]
    fn classifier_uses_host_and_path_without_network() {
        assert_eq!(classify("https://arxiv.org/abs/1234.1"), SourceClass::Paper);
        assert_eq!(classify("https://github.com/org/repo"), SourceClass::Github);
        assert_eq!(
            classify("https://stackoverflow.com/questions/1"),
            SourceClass::StackOverflow
        );
        assert_eq!(
            classify("https://example.gov/report"),
            SourceClass::Government
        );
        assert_eq!(classify("https://example.com/file.pdf"), SourceClass::Pdf);
        assert_eq!(
            classify("https://example.com/docs/start"),
            SourceClass::Docs
        );
    }

    #[test]
    fn pdf_profile_does_not_treat_binary_as_link_graph_input() {
        let profile = profile_for(SourceClass::Pdf);
        assert!(!profile.include_links);
        assert!(profile.favor_recall);
    }
}
