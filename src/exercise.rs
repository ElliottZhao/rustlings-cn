use regex::Regex;
use serde::Deserialize;
use std::env;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, remove_file, File};
use std::io::Read;
use std::path::PathBuf;
use std::process::{self, Command};

const RUSTC_COLOR_ARGS: &[&str] = &["--color", "always"];
const RUSTC_EDITION_ARGS: &[&str] = &["--edition", "2021"];
const I_AM_DONE_REGEX: &str = r"(?m)^\s*///?\s*I\s+AM\s+NOT\s+DONE";
const CONTEXT: usize = 2;
const CLIPPY_CARGO_TOML_PATH: &str = "./exercises/clippy/Cargo.toml";

// 获取一个希望是唯一的临时文件名
#[inline]
fn temp_file() -> String {
    let thread_id: String = format!("{:?}", std::thread::current().id())
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();

    format!("./temp_{}_{thread_id}", process::id())
}

// 练习的方式。
#[derive(Deserialize, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    // 指示练习应编译为二进制文件
    Compile,
    // 指示该练习应编译为测试工具
    Test,
    // 表示该练习应使用 Clippy 进行检查
    Clippy,
}

#[derive(Deserialize)]
pub struct ExerciseList {
    pub exercises: Vec<Exercise>,
}

// Rustlings练习的表现形式。
// 这是从随附的 info.toml 文件反序列化的
#[derive(Deserialize, Debug)]
pub struct Exercise {
    // 练习名称
    pub name: String,
    // 包含练习源代码的文件的路径
    pub path: PathBuf,
    // 练习的模式（Test、Compile或Clippy）
    pub mode: Mode,
    // 与练习相关的提示文本
    pub hint: String,
}

// 用于跟踪练习状态的枚举。
// 练习可以是已完成或待完成
#[derive(PartialEq, Debug)]
pub enum State {
    // 练习完成后的状态
    Done,
    // 尚未完成时的练习状态
    Pending(Vec<ContextLine>),
}

// 待处理练习的上下文信息
#[derive(PartialEq, Debug)]
pub struct ContextLine {
    // 待完成的源代码
    pub line: String,
    // 待完成的行号
    pub number: usize,
    // 是否重要
    pub important: bool,
}

// 编译练习的结果
pub struct CompiledExercise<'a> {
    exercise: &'a Exercise,
    _handle: FileHandle,
}

impl<'a> CompiledExercise<'a> {
    // 运行编译好的练习
    pub fn run(&self) -> Result<ExerciseOutput, ExerciseOutput> {
        self.exercise.run()
    }
}

// 已执行的二进制文件的表示
#[derive(Debug)]
pub struct ExerciseOutput {
    // 二进制文件标准输出的文本内容
    pub stdout: String,
    // 二进制文件标准错误的文本内容
    pub stderr: String,
}

struct FileHandle;

impl Drop for FileHandle {
    fn drop(&mut self) {
        clean();
    }
}

impl Exercise {
    pub fn compile(&self) -> Result<CompiledExercise, ExerciseOutput> {
        let cmd = match self.mode {
            Mode::Compile => Command::new("rustc")
                .args([self.path.to_str().unwrap(), "-o", &temp_file()])
                .args(RUSTC_COLOR_ARGS)
                .args(RUSTC_EDITION_ARGS)
                .output(),
            Mode::Test => Command::new("rustc")
                .args(["--test", self.path.to_str().unwrap(), "-o", &temp_file()])
                .args(RUSTC_COLOR_ARGS)
                .args(RUSTC_EDITION_ARGS)
                .output(),
            Mode::Clippy => {
                let cargo_toml = format!(
                    r#"[package]
name = "{}"
version = "0.0.1"
edition = "2021"
[[bin]]
name = "{}"
path = "{}.rs""#,
                    self.name, self.name, self.name
                );
                let cargo_toml_error_msg = if env::var("NO_EMOJI").is_ok() {
                    "无法写入 Clippy Cargo.toml 文件。"
                } else {
                    "无法写入 📎 Clippy 📎 Cargo.toml 文件。"
                };
                fs::write(CLIPPY_CARGO_TOML_PATH, cargo_toml).expect(cargo_toml_error_msg);
                // To support the ability to run the clippy exercises, build
                // an executable, in addition to running clippy. With a
                // compilation failure, this would silently fail. But we expect
                // clippy to reflect the same failure while compiling later.
                Command::new("rustc")
                    .args([self.path.to_str().unwrap(), "-o", &temp_file()])
                    .args(RUSTC_COLOR_ARGS)
                    .args(RUSTC_EDITION_ARGS)
                    .output()
                    .expect("编译失败！");
                // 由于 Clippy 存在问题，需要进行cargo clean以清除所有lints。
                // See https://github.com/rust-lang/rust-clippy/issues/2604
                // 这已经在 Clippy 的 master 分支上得到修复。 请参阅此问题以跟踪合并到 Cargo 中：
                // https://github.com/rust-lang/rust-clippy/issues/3837
                Command::new("cargo")
                    .args(["clean", "--manifest-path", CLIPPY_CARGO_TOML_PATH])
                    .args(RUSTC_COLOR_ARGS)
                    .output()
                    .expect("无法运行“cargo clean”");
                Command::new("cargo")
                    .args(["clippy", "--manifest-path", CLIPPY_CARGO_TOML_PATH])
                    .args(RUSTC_COLOR_ARGS)
                    .args(["--", "-D", "warnings", "-D", "clippy::float_cmp"])
                    .output()
            }
        }
        .expect("无法运行“compile”命令。");

        if cmd.status.success() {
            Ok(CompiledExercise {
                exercise: self,
                _handle: FileHandle,
            })
        } else {
            clean();
            Err(ExerciseOutput {
                stdout: String::from_utf8_lossy(&cmd.stdout).to_string(),
                stderr: String::from_utf8_lossy(&cmd.stderr).to_string(),
            })
        }
    }

    fn run(&self) -> Result<ExerciseOutput, ExerciseOutput> {
        let arg = match self.mode {
            Mode::Test => "--show-output",
            _ => "",
        };
        let cmd = Command::new(temp_file())
            .arg(arg)
            .output()
            .expect("无法运行“run”命令");

        let output = ExerciseOutput {
            stdout: String::from_utf8_lossy(&cmd.stdout).to_string(),
            stderr: String::from_utf8_lossy(&cmd.stderr).to_string(),
        };

        if cmd.status.success() {
            Ok(output)
        } else {
            Err(output)
        }
    }

    pub fn state(&self) -> State {
        let mut source_file =
            File::open(&self.path).expect("我们无法打开练习文件！");

        let source = {
            let mut s = String::new();
            source_file
                .read_to_string(&mut s)
                .expect("我们无法读取练习文件！");
            s
        };

        let re = Regex::new(I_AM_DONE_REGEX).unwrap();

        if !re.is_match(&source) {
            return State::Done;
        }

        let matched_line_index = source
            .lines()
            .enumerate()
            .find_map(|(i, line)| if re.is_match(line) { Some(i) } else { None })
            .expect("这根本不应该发生");

        let min_line = ((matched_line_index as i32) - (CONTEXT as i32)).max(0) as usize;
        let max_line = matched_line_index + CONTEXT;

        let context = source
            .lines()
            .enumerate()
            .filter(|&(i, _)| i >= min_line && i <= max_line)
            .map(|(i, line)| ContextLine {
                line: line.to_string(),
                number: i + 1,
                important: i == matched_line_index,
            })
            .collect();

        State::Pending(context)
    }

    // 使用 self.state() 检查练习是否已解决
    // 这不是最好的检查方法，因为用户可以从文件中删除“I AM NOT DONE”字符串，
    // 而实际上没有解决任何问题。
    // 真正检查这一点的唯一其他方法是编译并运行练习； 这既昂贵又违反直觉
    pub fn looks_done(&self) -> bool {
        self.state() == State::Done
    }
}

impl Display for Exercise {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.path.to_str().unwrap())
    }
}

#[inline]
fn clean() {
    let _ignored = remove_file(temp_file());
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_clean() {
        File::create(temp_file()).unwrap();
        let exercise = Exercise {
            name: String::from("example"),
            path: PathBuf::from("tests/fixture/state/pending_exercise.rs"),
            mode: Mode::Compile,
            hint: String::from(""),
        };
        let compiled = exercise.compile().unwrap();
        drop(compiled);
        assert!(!Path::new(&temp_file()).exists());
    }

    #[test]
    fn test_pending_state() {
        let exercise = Exercise {
            name: "pending_exercise".into(),
            path: PathBuf::from("tests/fixture/state/pending_exercise.rs"),
            mode: Mode::Compile,
            hint: String::new(),
        };

        let state = exercise.state();
        let expected = vec![
            ContextLine {
                line: "// fake_exercise".to_string(),
                number: 1,
                important: false,
            },
            ContextLine {
                line: "".to_string(),
                number: 2,
                important: false,
            },
            ContextLine {
                line: "// I AM NOT DONE".to_string(),
                number: 3,
                important: true,
            },
            ContextLine {
                line: "".to_string(),
                number: 4,
                important: false,
            },
            ContextLine {
                line: "fn main() {".to_string(),
                number: 5,
                important: false,
            },
        ];

        assert_eq!(state, State::Pending(expected));
    }

    #[test]
    fn test_finished_exercise() {
        let exercise = Exercise {
            name: "finished_exercise".into(),
            path: PathBuf::from("tests/fixture/state/finished_exercise.rs"),
            mode: Mode::Compile,
            hint: String::new(),
        };

        assert_eq!(exercise.state(), State::Done);
    }

    #[test]
    fn test_exercise_with_output() {
        let exercise = Exercise {
            name: "exercise_with_output".into(),
            path: PathBuf::from("tests/fixture/success/testSuccess.rs"),
            mode: Mode::Test,
            hint: String::new(),
        };
        let out = exercise.compile().unwrap().run().unwrap();
        assert!(out.stdout.contains("THIS TEST TOO SHALL PASS"));
    }
}
