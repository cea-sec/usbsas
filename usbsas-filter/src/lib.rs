//! usbsas's name filter process. filter can prevent the copy of certain files
//! based on their names (for example ".DS_STORE", "AUTORUN.INF" etc.). Filters
//! can be specified in the configuration file.

use log::debug;
#[cfg(test)]
use serde::{Deserialize, Serialize};
use thiserror::Error;
use usbsas_comm::{ComRpFilter, ProtoRespCommon, ProtoRespFilter, SendRecv, ToFromFd};
use usbsas_config::{conf_parse, conf_read};
use usbsas_proto as proto;
use usbsas_proto::{filter::request::Msg, filter::FilterResult};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Error(String),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("State error")]
    State,
}
pub type Result<T> = std::result::Result<T, Error>;

#[cfg_attr(test, derive(Serialize, Deserialize))]
pub struct Rule {
    contain: Option<Vec<String>>,
    start: Option<String>,
    end: Option<String>,
}

impl Rule {
    fn into_lowercase(self) -> Self {
        Rule {
            contain: self
                .contain
                .map(|v| v.iter().map(|s| s.to_lowercase()).collect()),
            start: self.start.map(|v| v.to_lowercase()),
            end: self.end.map(|v| v.to_lowercase()),
        }
    }

    fn match_(&self, input: &str) -> bool {
        let input = input.to_lowercase();
        if let Some(ref contain) = self.contain {
            for pattern in contain.iter() {
                if !input.contains(pattern) {
                    return false;
                }
            }
        }
        if let Some(ref start) = self.start {
            if !input.starts_with(start) {
                return false;
            }
        }
        if let Some(ref end) = self.end {
            if !input.ends_with(end) {
                return false;
            }
        }
        true
    }
}

#[cfg_attr(test, derive(Serialize, Deserialize))]
pub struct Rules {
    rules: Vec<Rule>,
}

impl Rules {
    fn into_lowercase(self) -> Self {
        Rules {
            rules: self.rules.into_iter().map(|f| f.into_lowercase()).collect(),
        }
    }

    fn match_all(&self, input: &str) -> FilterResult {
        for f in self.rules.iter() {
            if f.match_(input) {
                return FilterResult::PathFiltered;
            }
        }
        FilterResult::PathOk
    }
}

enum State {
    Init(InitState),
    Running(RunningState),
    End,
}

impl State {
    fn run(self, comm: &mut ComRpFilter) -> Result<Self> {
        match self {
            State::Init(s) => s.run(comm),
            State::Running(s) => s.run(comm),
            State::End => Err(Error::State),
        }
    }
}

struct InitState {
    config_path: String,
}
struct RunningState {
    rules: Rules,
}

impl InitState {
    fn run(self, comm: &mut ComRpFilter) -> Result<State> {
        let config_str = conf_read(&self.config_path)?;

        usbsas_sandbox::filter::seccomp(comm.input_fd(), comm.output_fd())?;

        let config = conf_parse(&config_str)?;
        let rules = config
            .filters
            .unwrap_or_default()
            .into_iter()
            .map(|f| Rule {
                contain: f.contain,
                start: f.start,
                end: f.end,
            })
            .collect();

        let rules = Rules { rules }.into_lowercase();
        Ok(State::Running(RunningState { rules }))
    }
}

impl RunningState {
    fn run(self, comm: &mut ComRpFilter) -> Result<State> {
        loop {
            let req: proto::filter::Request = comm.recv()?;
            match req.msg.ok_or(Error::BadRequest)? {
                Msg::FilterPaths(req) => self.filterpaths(comm, req.path)?,
                Msg::End(_) => {
                    comm.end()?;
                    break;
                }
            }
        }
        Ok(State::End)
    }

    fn filterpaths(&self, comm: &mut ComRpFilter, paths: Vec<String>) -> Result<()> {
        let results = paths
            .iter()
            .map(|p| self.rules.match_all(p) as i32)
            .collect();
        debug!("filter results {:?}", results);
        comm.filterpaths(proto::filter::ResponseFilterPaths { results })?;
        Ok(())
    }
}

pub struct Filter {
    comm: ComRpFilter,
    state: State,
}

impl Filter {
    pub fn new(comm: ComRpFilter, config_path: String) -> Result<Self> {
        Ok(Filter {
            comm,
            state: State::Init(InitState { config_path }),
        })
    }

    pub fn main_loop(self) -> Result<()> {
        let (mut comm, mut state) = (self.comm, self.state);
        loop {
            state = match state.run(&mut comm)? {
                State::End => break,
                state => state,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Rules;
    use usbsas_proto::filter::FilterResult;

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
"#;

    #[test]
    fn test_filters_from_config() {
        let rules: Rules = toml::from_str(CONF).expect("can't parse toml");
        let rules = rules.into_lowercase();
        assert_eq!(rules.match_all("good"), FilterResult::PathOk);
        assert_eq!(rules.match_all("bad.lnk"), FilterResult::PathFiltered);
        assert_eq!(rules.match_all("good.lnk.not_ending"), FilterResult::PathOk);
        assert_eq!(
            rules.match_all("X frag1 frag2 Y"),
            FilterResult::PathFiltered
        );
        assert_eq!(rules.match_all("X frag3 frag4 Y"), FilterResult::PathOk);
        assert_eq!(rules.match_all(".bad"), FilterResult::PathFiltered);
        assert_eq!(rules.match_all("not_starting.bad"), FilterResult::PathOk);
        assert_eq!(rules.match_all(".__MACOSX"), FilterResult::PathFiltered);
        assert_eq!(rules.match_all(".DS_Store"), FilterResult::PathFiltered);
    }
}
