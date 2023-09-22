use crate::exercise::{Exercise, ExerciseList};
use crate::project::RustAnalyzerProject;
use crate::run::{reset, run};
use crate::verify::verify;
use clap::{Parser, Subcommand};
use console::Emoji;
use notify::DebouncedEvent;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, prelude::*};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[macro_use]
mod ui;

mod exercise;
mod project;
mod run;
mod verify;

/// Rustlings is a collection of small exercises to get you used to writing and reading Rust code
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Show outputs from the test exercises
    #[arg(long)]
    nocapture: bool,
    #[command(subcommand)]
    command: Option<Subcommands>,
}

#[derive(Subcommand)]
enum Subcommands {
    /// Verify all exercises according to the recommended order
    Verify,
    /// Rerun `verify` when files were edited
    Watch {
        /// Show hints on success
        #[arg(long)]
        success_hints: bool,
    },
    /// Run/Test a single exercise
    Run {
        /// The name of the exercise
        name: String,
    },
    /// Reset a single exercise using "git stash -- <filename>"
    Reset {
        /// The name of the exercise
        name: String,
    },
    /// Return a hint for the given exercise
    Hint {
        /// The name of the exercise
        name: String,
    },
    /// List the exercises available in Rustlings
    List {
        /// Show only the paths of the exercises
        #[arg(short, long)]
        paths: bool,
        /// Show only the names of the exercises
        #[arg(short, long)]
        names: bool,
        /// Provide a string to match exercise names.
        /// Comma separated patterns are accepted
        #[arg(short, long)]
        filter: Option<String>,
        /// Display only exercises not yet solved
        #[arg(short, long)]
        unsolved: bool,
        /// Display only exercises that have been solved
        #[arg(short, long)]
        solved: bool,
    },
    /// Enable rust-analyzer for exercises
    Lsp,
}

fn main() {
    let args = Args::parse();

    if args.command.is_none() {
        println!("\n{WELCOME}\n");
    }

    if !Path::new("info.toml").exists() {
        println!(
            "{}必须从 rusdlings 目录运行",
            std::env::current_exe().unwrap().to_str().unwrap()
        );
        println!("试试`cd rustlings/`!");
        std::process::exit(1);
    }

    if !rustc_exists() {
        println!("我们找不到 `rustc`。");
        println!("尝试运行“rustc --version”来诊断您的问题。");
        println!("有关如何安装 Rust 的说明，请查看 README。");
        std::process::exit(1);
    }

    let toml_str = &fs::read_to_string("info.toml").unwrap();
    let exercises = toml::from_str::<ExerciseList>(toml_str).unwrap().exercises;
    let verbose = args.nocapture;

    let command = args.command.unwrap_or_else(|| {
        println!("{DEFAULT_OUT}\n");
        std::process::exit(0);
    });

    match command {
        Subcommands::List {
            paths,
            names,
            filter,
            unsolved,
            solved,
        } => {
            if !paths && !names {
                println!("{:<17}\t{:<46}\t{:<7}", "名称", "路径", "状态");
            }
            let mut exercises_done: u16 = 0;
            let filters = filter.clone().unwrap_or_default().to_lowercase();
            exercises.iter().for_each(|e| {
                let fname = format!("{}", e.path.display());
                let filter_cond = filters
                    .split(',')
                    .filter(|f| !f.trim().is_empty())
                    .any(|f| e.name.contains(f) || fname.contains(f));
                let status = if e.looks_done() {
                    exercises_done += 1;
                    "完成"
                } else {
                    "待办"
                };
                let solve_cond = {
                    (e.looks_done() && solved)
                        || (!e.looks_done() && unsolved)
                        || (!solved && !unsolved)
                };
                if solve_cond && (filter_cond || filter.is_none()) {
                    let line = if paths {
                        format!("{fname}\n")
                    } else if names {
                        format!("{}\n", e.name)
                    } else {
                        format!("{:<17}\t{fname:<46}\t{status:<7}\n", e.name)
                    };
                    // Somehow using println! leads to the binary panicking
                    // when its output is piped.
                    // So, we're handling a Broken Pipe error and exiting with 0 anyway
                    let stdout = std::io::stdout();
                    {
                        let mut handle = stdout.lock();
                        handle.write_all(line.as_bytes()).unwrap_or_else(|e| {
                            match e.kind() {
                                std::io::ErrorKind::BrokenPipe => std::process::exit(0),
                                _ => std::process::exit(1),
                            };
                        });
                    }
                }
            });
            let percentage_progress = exercises_done as f32 / exercises.len() as f32 * 100.0;
            println!(
                "进度：您完成了 {} / {} 个练习 ({:.1} %)。",
                exercises_done,
                exercises.len(),
                percentage_progress
            );
            std::process::exit(0);
        }

        Subcommands::Run { name } => {
            let exercise = find_exercise(&name, &exercises);

            run(exercise, verbose).unwrap_or_else(|_| std::process::exit(1));
        }

        Subcommands::Reset { name } => {
            let exercise = find_exercise(&name, &exercises);

            reset(exercise).unwrap_or_else(|_| std::process::exit(1));
        }

        Subcommands::Hint { name } => {
            let exercise = find_exercise(&name, &exercises);

            println!("{}", exercise.hint);
        }

        Subcommands::Verify => {
            verify(&exercises, (0, exercises.len()), verbose, false)
                .unwrap_or_else(|_| std::process::exit(1));
        }

        Subcommands::Lsp => {
            let mut project = RustAnalyzerProject::new();
            project
                .get_sysroot_src()
                .expect("找不到工具链路径，您是否安装了“rustc”？");
            project
                .exercises_to_json()
                .expect("无法解析 rusdlings 练习文件");

            if project.crates.is_empty() {
                println!("找不到任何练习，请确保您位于“rusdlings”文件夹中");
            } else if project.write_to_disk().is_err() {
                println!("无法将 rust-project.json 写入 rust-analyzer 的磁盘");
            } else {
                println!("成功生成rust-project.json");
                println!("rust-analyzer 现在将解析练习，重新启动语言服务器或编辑器")
            }
        }

        Subcommands::Watch { success_hints } => match watch(&exercises, verbose, success_hints) {
            Err(e) => {
                println!(
                    "错误：无法查看您的进度。 错误消息为 {:?}。",
                    e
                );
                println!("您很可能已经用完磁盘空间或已达到“inotify 限制”。");
                std::process::exit(1);
            }
            Ok(WatchStatus::Finished) => {
                println!(
                    "{emoji} 所有练习完成！ {emoji}",
                    emoji = Emoji("🎉", "★")
                );
                println!("\n{FENISH_LINE}\n");
            }
            Ok(WatchStatus::Unfinished) => {
                println!("我们希望您喜欢学习 Rust！");
                println!("如果您想稍后继续进行练习，只需再次运行“ruslings watch”即可");
            }
        },
    }
}

fn spawn_watch_shell(
    failed_exercise_hint: &Arc<Mutex<Option<String>>>,
    should_quit: Arc<AtomicBool>,
) {
    let failed_exercise_hint = Arc::clone(failed_exercise_hint);
    println!("欢迎来到观察模式！ 您可以输入“help”来获取可在此处使用的命令的概述。");
    thread::spawn(move || loop {
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(_) => {
                let input = input.trim();
                if input == "hint" {
                    if let Some(hint) = &*failed_exercise_hint.lock().unwrap() {
                        println!("{hint}");
                    }
                } else if input == "clear" {
                    println!("\x1B[2J\x1B[1;1H");
                } else if input.eq("quit") {
                    should_quit.store(true, Ordering::SeqCst);
                    println!("再见");
                } else if input.eq("help") {
                    println!("观察模式下可用的命令：");
                    println!("  hint   - 打印当前练习的提示");
                    println!("  clear  - 清除屏幕");
                    println!("  quit   - 退出观察模式");
                    println!("  !<cmd> - 执行命令，如 `!rustc --explain E0381`");
                    println!("  help   - 显示此帮助消息");
                    println!();
                    println!("当您编辑文件内容时，观察模式会自动重新评估当前练习。");
                } else if let Some(cmd) = input.strip_prefix('!') {
                    let parts: Vec<&str> = cmd.split_whitespace().collect();
                    if parts.is_empty() {
                        println!("没有提供命令");
                    } else if let Err(e) = Command::new(parts[0]).args(&parts[1..]).status() {
                        println!("执行命令失败 `{}`: {}", cmd, e);
                    }
                } else {
                    println!("未知命令: {input}");
                }
            }
            Err(error) => println!("error reading command: {error}"),
        }
    });
}

fn find_exercise<'a>(name: &str, exercises: &'a [Exercise]) -> &'a Exercise {
    if name.eq("next") {
        exercises
            .iter()
            .find(|e| !e.looks_done())
            .unwrap_or_else(|| {
                println!("🎉 恭喜！ 您已完成所有练习！");
                println!("🔚 接下来没有更多的练习可以做！");
                std::process::exit(1)
            })
    } else {
        exercises
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| {
                println!("找不到'{name}'的练习！");
                std::process::exit(1)
            })
    }
}

enum WatchStatus {
    Finished,
    Unfinished,
}

fn watch(
    exercises: &[Exercise],
    verbose: bool,
    success_hints: bool,
) -> notify::Result<WatchStatus> {
    /* Clears the terminal with an ANSI escape code.
    Works in UNIX and newer Windows terminals. */
    fn clear_screen() {
        println!("\x1Bc");
    }

    let (tx, rx) = channel();
    let should_quit = Arc::new(AtomicBool::new(false));

    let mut watcher: RecommendedWatcher = Watcher::new(tx, Duration::from_secs(1))?;
    watcher.watch(Path::new("./exercises"), RecursiveMode::Recursive)?;

    clear_screen();

    let to_owned_hint = |t: &Exercise| t.hint.to_owned();
    let failed_exercise_hint = match verify(
        exercises.iter(),
        (0, exercises.len()),
        verbose,
        success_hints,
    ) {
        Ok(_) => return Ok(WatchStatus::Finished),
        Err(exercise) => Arc::new(Mutex::new(Some(to_owned_hint(exercise)))),
    };
    spawn_watch_shell(&failed_exercise_hint, Arc::clone(&should_quit));
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => match event {
                DebouncedEvent::Create(b) | DebouncedEvent::Chmod(b) | DebouncedEvent::Write(b) => {
                    if b.extension() == Some(OsStr::new("rs")) && b.exists() {
                        let filepath = b.as_path().canonicalize().unwrap();
                        let pending_exercises = exercises
                            .iter()
                            .find(|e| filepath.ends_with(&e.path))
                            .into_iter()
                            .chain(
                                exercises
                                    .iter()
                                    .filter(|e| !e.looks_done() && !filepath.ends_with(&e.path)),
                            );
                        let num_done = exercises.iter().filter(|e| e.looks_done()).count();
                        clear_screen();
                        match verify(
                            pending_exercises,
                            (num_done, exercises.len()),
                            verbose,
                            success_hints,
                        ) {
                            Ok(_) => return Ok(WatchStatus::Finished),
                            Err(exercise) => {
                                let mut failed_exercise_hint = failed_exercise_hint.lock().unwrap();
                                *failed_exercise_hint = Some(to_owned_hint(exercise));
                            }
                        }
                    }
                }
                _ => {}
            },
            Err(RecvTimeoutError::Timeout) => {
                // the timeout expired, just check the `should_quit` variable below then loop again
            }
            Err(e) => println!("watch error: {e:?}"),
        }
        // Check if we need to exit
        if should_quit.load(Ordering::SeqCst) {
            return Ok(WatchStatus::Unfinished);
        }
    }
}

fn rustc_exists() -> bool {
    Command::new("rustc")
        .args(["--version"])
        .stdout(Stdio::null())
        .spawn()
        .and_then(|mut child| child.wait())
        .map(|status| status.success())
        .unwrap_or(false)
}

const DEFAULT_OUT: &str = r#"感谢您安装 Rustlings！

这是你第一次吗？ 别担心，Ruslings 是为初学者而设计的！ 我们将教您很多有关 Rust 的知识，
但在开始之前，这里有一些关于 Rustlings 运作方式的说明：

1. Rustlings 背后的核心概念是解决练习。 这些练习通常存在某种语法错误，这将导致它们编译
   或测试失败。 有时存在逻辑错误而不是语法错误。 无论什么错误，找到它并修复它都是你的工作！
   当你修复它时你就会知道，因为那时练习将编译并且 Rustlings 将能够继续下一个练习。
2. 如果您在观察模式下运行 Rustlings（我们推荐），它将自动开始第一个练习。 不要因为运行
   Rustlings 后立即弹出的错误消息而感到困惑！ 这是您应该解决的练习的一部分，因此在编辑器
   中打开练习文件并开始您的侦探工作！
3. 如果您在某项练习中遇到困难，可以通过键入“hint”（在观察模式下）或运行
   “rusdlingshintexercise_name”来查看有用的提示。
4. 如果某个练习对您来说没有意义，请随时在 GitHub 上提出问题！
   （https://github.com/rust-lang/rustlings/issues/new）。
   我们会研究每个问题，有时其他学习者也会这样做，这样你们就可以互相帮助！
5. 如果您想在练习中使用“rust-analyzer”（它提供自动完成等功能），请运行命令“rustlings lsp”。

明白了吗？ 伟大的！ 首先，运行“ruslings watch”以获得第一个练习。 确保打开你的编辑器！"#;

const FENISH_LINE: &str = r"+----------------------------------------------------+
|          您已到达终点线！          |
+--------------------------  ------------------------+
                          \\/
     ▒▒          ▒▒▒▒▒▒▒▒      ▒▒▒▒▒▒▒▒          ▒▒
   ▒▒▒▒  ▒▒    ▒▒        ▒▒  ▒▒        ▒▒    ▒▒  ▒▒▒▒
   ▒▒▒▒  ▒▒  ▒▒            ▒▒            ▒▒  ▒▒  ▒▒▒▒
 ░░▒▒▒▒░░▒▒  ▒▒            ▒▒            ▒▒  ▒▒░░▒▒▒▒
   ▓▓▓▓▓▓▓▓  ▓▓      ▓▓██  ▓▓  ▓▓██      ▓▓  ▓▓▓▓▓▓▓▓
     ▒▒▒▒    ▒▒      ████  ▒▒  ████      ▒▒░░  ▒▒▒▒
       ▒▒  ▒▒▒▒▒▒        ▒▒▒▒▒▒        ▒▒▒▒▒▒  ▒▒
         ▒▒▒▒▒▒▒▒▒▒▓▓▓▓▓▓▒▒▒▒▒▒▒▒▓▓▒▒▓▓▒▒▒▒▒▒▒▒
           ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒
             ▒▒▒▒▒▒▒▒▒▒██▒▒▒▒▒▒██▒▒▒▒▒▒▒▒▒▒
           ▒▒  ▒▒▒▒▒▒▒▒▒▒██████▒▒▒▒▒▒▒▒▒▒  ▒▒
         ▒▒    ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒    ▒▒
       ▒▒    ▒▒    ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒    ▒▒    ▒▒
       ▒▒  ▒▒    ▒▒                  ▒▒    ▒▒  ▒▒
           ▒▒  ▒▒                      ▒▒  ▒▒

我们希望您喜欢学习 Rust 的各个方面！
如果您发现任何问题，请随时向我们的存储库报告。
您还可以贡献自己的练习来帮助更大的社区！

在报告问题或做出贡献之前，请阅读我们的指南：
https://github.com/rust-lang/rustlings/blob/main/CONTRIBUTING.md";

const WELCOME: &str = r"       欢迎来到……
                 _   _ _
  _ __ _   _ ___| |_| (_)_ __   __ _ ___
 | '__| | | / __| __| | | '_ \ / _` / __|
 | |  | |_| \__ \ |_| | | | | | (_| \__ \
 |_|   \__,_|___/\__|_|_|_| |_|\__, |___/
                               |___/";
