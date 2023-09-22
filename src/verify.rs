use crate::exercise::{CompiledExercise, Exercise, Mode, State};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::{env, time::Duration};

// Verify that the provided container of Exercise objects
// can be compiled and run without any failures.
// Any such failures will be reported to the end user.
// If the Exercise being verified is a test, the verbose boolean
// determines whether or not the test harness outputs are displayed.
pub fn verify<'a>(
    exercises: impl IntoIterator<Item = &'a Exercise>,
    progress: (usize, usize),
    verbose: bool,
    success_hints: bool,
) -> Result<(), &'a Exercise> {
    let (num_done, total) = progress;
    let bar = ProgressBar::new(total as u64);
    let mut percentage = num_done as f32 / total as f32 * 100.0;
    bar.set_style(
        ProgressStyle::default_bar()
            .template("进度: [{bar:60.green/red}] {pos}/{len} {msg}")
            .expect("进度条模板应该有效！")
            .progress_chars("#>-"),
    );
    bar.set_position(num_done as u64);
    bar.set_message(format!("({:.1} %)", percentage));

    for exercise in exercises {
        let compile_result = match exercise.mode {
            Mode::Test => compile_and_test(exercise, RunMode::Interactive, verbose, success_hints),
            Mode::Compile => compile_and_run_interactively(exercise, success_hints),
            Mode::Clippy => compile_only(exercise, success_hints),
        };
        if !compile_result.unwrap_or(false) {
            return Err(exercise);
        }
        percentage += 100.0 / total as f32;
        bar.inc(1);
        bar.set_message(format!("({:.1} %)", percentage));
    }
    Ok(())
}

enum RunMode {
    Interactive,
    NonInteractive,
}

// Compile and run the resulting test harness of the given Exercise
pub fn test(exercise: &Exercise, verbose: bool) -> Result<(), ()> {
    compile_and_test(exercise, RunMode::NonInteractive, verbose, false)?;
    Ok(())
}

// Invoke the rust compiler without running the resulting binary
fn compile_only(exercise: &Exercise, success_hints: bool) -> Result<bool, ()> {
    let progress_bar = ProgressBar::new_spinner();
    progress_bar.set_message(format!("编译 {exercise} 中……"));
    progress_bar.enable_steady_tick(Duration::from_millis(100));

    let _ = compile(exercise, &progress_bar)?;
    progress_bar.finish_and_clear();

    Ok(prompt_for_completion(exercise, None, success_hints))
}

// Compile the given Exercise and run the resulting binary in an interactive mode
fn compile_and_run_interactively(exercise: &Exercise, success_hints: bool) -> Result<bool, ()> {
    let progress_bar = ProgressBar::new_spinner();
    progress_bar.set_message(format!("编译 {exercise} 中……"));
    progress_bar.enable_steady_tick(Duration::from_millis(100));

    let compilation = compile(exercise, &progress_bar)?;

    progress_bar.set_message(format!("运行 {exercise} 中……"));
    let result = compilation.run();
    progress_bar.finish_and_clear();

    let output = match result {
        Ok(output) => output,
        Err(output) => {
            warn!("运行 {} 但出现错误", exercise);
            println!("{}", output.stdout);
            println!("{}", output.stderr);
            return Err(());
        }
    };

    Ok(prompt_for_completion(
        exercise,
        Some(output.stdout),
        success_hints,
    ))
}

// Compile the given Exercise as a test harness and display
// the output if verbose is set to true
fn compile_and_test(
    exercise: &Exercise,
    run_mode: RunMode,
    verbose: bool,
    success_hints: bool,
) -> Result<bool, ()> {
    let progress_bar = ProgressBar::new_spinner();
    progress_bar.set_message(format!("测试 {exercise} 中……"));
    progress_bar.enable_steady_tick(Duration::from_millis(100));

    let compilation = compile(exercise, &progress_bar)?;
    let result = compilation.run();
    progress_bar.finish_and_clear();

    match result {
        Ok(output) => {
            if verbose {
                println!("{}", output.stdout);
            }
            if let RunMode::Interactive = run_mode {
                Ok(prompt_for_completion(exercise, None, success_hints))
            } else {
                Ok(true)
            }
        }
        Err(output) => {
            warn!(
                "{} 测试失败！请再试一次。这是输出：",
                exercise
            );
            println!("{}", output.stdout);
            Err(())
        }
    }
}

// Compile the given Exercise and return an object with information
// about the state of the compilation
fn compile<'a>(
    exercise: &'a Exercise,
    progress_bar: &ProgressBar,
) -> Result<CompiledExercise<'a>, ()> {
    let compilation_result = exercise.compile();

    match compilation_result {
        Ok(compilation) => Ok(compilation),
        Err(output) => {
            progress_bar.finish_and_clear();
            warn!(
                "{} 编译失败！请再试一次。这是输出：",
                exercise
            );
            println!("{}", output.stderr);
            Err(())
        }
    }
}

fn prompt_for_completion(
    exercise: &Exercise,
    prompt_output: Option<String>,
    success_hints: bool,
) -> bool {
    let context = match exercise.state() {
        State::Done => return true,
        State::Pending(context) => context,
    };
    match exercise.mode {
        Mode::Compile => success!("成功运行{}！", exercise),
        Mode::Test => success!("成功测试{}！", exercise),
        Mode::Clippy => success!("成功编译{}！", exercise),
    }

    let no_emoji = env::var("NO_EMOJI").is_ok();

    let clippy_success_msg = if no_emoji {
        "代码正在编译，Clippy 很高兴！"
    } else {
        "代码正在编译，📎 Clippy 📎很高兴！"
    };

    let success_msg = match exercise.mode {
        Mode::Compile => "代码正在编译！",
        Mode::Test => "代码正在编译，测试通过！",
        Mode::Clippy => clippy_success_msg,
    };
    println!();
    if no_emoji {
        println!("~*~ {success_msg} ~*~")
    } else {
        println!("🎉 🎉  {success_msg} 🎉 🎉")
    }
    println!();

    if let Some(output) = prompt_output {
        println!("输出：");
        println!("{}", separator());
        println!("{output}");
        println!("{}", separator());
        println!();
    }
    if success_hints {
        println!("提示：");
        println!("{}", separator());
        println!("{}", exercise.hint);
        println!("{}", separator());
        println!();
    }

    println!(
        "您可以继续进行此练习，或者通过删除 {} 注释跳到下一个练习：",
        style("`I AM NOT DONE`").bold()
    );
    println!();
    for context_line in context {
        let formatted_line = if context_line.important {
            format!("{}", style(context_line.line).bold())
        } else {
            context_line.line.to_string()
        };

        println!(
            "{:>2} {}  {}",
            style(context_line.number).blue().bold(),
            style("|").blue(),
            formatted_line
        );
    }

    false
}

fn separator() -> console::StyledObject<&'static str> {
    style("====================").bold()
}
