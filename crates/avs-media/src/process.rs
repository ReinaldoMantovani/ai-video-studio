//! Criação de processos filhos sem janela de console.
//!
//! No Windows, um app gráfico que inicia um programa de console (FFmpeg, Python)
//! faz o Windows abrir uma janela de terminal para ele, que pisca na tela a
//! cada chamada. `CREATE_NO_WINDOW` impede isso. Todo processo filho do app
//! deve ser criado por [`command`].

use std::ffi::OsStr;
use std::process::Command;

/// `CREATE_NO_WINDOW` (winbase.h).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Igual a `Command::new`, mas sem janela de console no Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}
