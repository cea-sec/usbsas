//! File name filters, from the `filters` section of the configuration.
//! Matching files are excluded from transfers.

#[cfg(test)]
use serde::{Deserialize, Serialize};
use usbsas_config::Filter;

/// A filter rule. A path matches if it satisfies every condition that is set
/// (case insensitive).
#[cfg_attr(test, derive(Serialize, Deserialize))]
pub struct Rule {
    /// Path contains all these strings
    pub contain: Option<Vec<String>>,
    /// Path starts with this string
    pub start: Option<String>,
    /// Path ends with this string
    pub end: Option<String>,
    /// Path is exactly this string
    pub exact: Option<String>,
}

impl From<Filter> for Rule {
    fn from(filter: Filter) -> Self {
        Self {
            contain: filter.contain,
            start: filter.start,
            end: filter.end,
            exact: filter.exact,
        }
    }
}

impl Rule {
    fn into_lowercase(self) -> Self {
        Rule {
            contain: self
                .contain
                .map(|v| v.iter().map(|s| s.to_lowercase()).collect()),
            start: self.start.map(|v| v.to_lowercase()),
            end: self.end.map(|v| v.to_lowercase()),
            exact: self.exact.map(|v| v.to_lowercase()),
        }
    }

    /// Whether `input` matches this rule, the rule must be lowercase.
    fn match_(&self, input: &str) -> bool {
        let input = input.to_lowercase();
        if let Some(ref exact) = self.exact
            && input != *exact
        {
            return false;
        }
        if let Some(ref contain) = self.contain {
            for pattern in contain.iter() {
                if !input.contains(pattern) {
                    return false;
                }
            }
        }
        if let Some(ref start) = self.start
            && !input.starts_with(start)
        {
            return false;
        }
        if let Some(ref end) = self.end
            && !input.ends_with(end)
        {
            return false;
        }
        true
    }
}

/// A set of filter rules.
#[cfg_attr(test, derive(Serialize, Deserialize))]
pub struct Rules {
    pub rules: Vec<Rule>,
}

impl From<Vec<Filter>> for Rules {
    fn from(pf: Vec<Filter>) -> Self {
        Self {
            rules: pf.into_iter().map(Rule::from).collect(),
        }
    }
}

impl Rules {
    /// Lowercase all rules, required before matching.
    pub fn into_lowercase(self) -> Self {
        Rules {
            rules: self.rules.into_iter().map(|f| f.into_lowercase()).collect(),
        }
    }

    /// Return true if a filename matches a rule
    pub fn match_all(&self, input: &str) -> bool {
        for f in self.rules.iter() {
            if f.match_(input) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::filter::Rules;
    use toml;

    const CONF: &str = r#"
[[rules]]
contain = ["__MACOSX"]

[[rules]]
contain = ["frag1", "frag2"]
start = "X"
end = "Y"

[[rules]]
start = ".bad"

[[rules]]
start = ".DS"
end = "_Store"

[[rules]]
end = ".lnk"

[[rules]]
exact = "lorem"

[[rules]]
exact = "lorem ipsum"

"#;

    #[test]
    fn test_filters_from_config() {
        let rules: Rules = toml::from_str(CONF).expect("can't parse toml");
        let rules = rules.into_lowercase();
        assert!(!rules.match_all("good"));
        assert!(rules.match_all("bad.lnk"));
        assert!(!rules.match_all("good.lnk.not_ending"));
        assert!(rules.match_all("X frag1 frag2 Y"));
        assert!(!rules.match_all("X frag3 frag4 Y"));
        assert!(rules.match_all(".bad"));
        assert!(!rules.match_all("not_starting.bad"));
        assert!(rules.match_all(".__MACOSX"));
        assert!(rules.match_all(".DS_Store"));
        assert!(rules.match_all("lorem"));
        assert!(!rules.match_all("ipsum"));
    }
}
