use eyre::Context;
use hanabot::{Hanabi, MessageProxy};
use slack_morphism::SlackUserId;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let mut hanabi = Hanabi::resume()
        .await
        .context("resume from saved game states")?
        .unwrap_or_default();

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    let mut msgs = StdoutMessageProxy::default();
    while let Some(line) = lines.next_line().await.context("read line from stdin")? {
        let (user, dm) = line
            .split_once(':')
            .ok_or_else(|| eyre::eyre!("line did not start with `user:`"))?;
        let dm = dm.trim();
        hanabi
            .on_dm_recv(dm, SlackUserId(user.to_string()), &mut msgs)
            .await
            .context("handle command as received dm")?;
        msgs.flush().await.context("flush responses")?;
    }

    hanabi.save().await
}

#[derive(Debug, Default)]
struct StdoutMessageProxy {
    msgs: HashMap<String, Vec<String>>,
}

impl StdoutMessageProxy {
    async fn flush(&mut self) -> eyre::Result<()> {
        let mut stdout = tokio::io::BufWriter::new(tokio::io::stdout());
        for (user, msgs) in self.msgs.drain() {
            for msg in msgs {
                for line in msg.lines() {
                    stdout
                        .write(format!("@{user} {line}\n").as_bytes())
                        .await
                        .with_context(|| format!("write out line of {user}'s responses"))?;
                }
            }
        }

        stdout.flush().await.context("flush bufwriter")?;

        Ok(())
    }
}

impl MessageProxy for StdoutMessageProxy {
    fn send(&mut self, user: &str, text: &str) {
        self.msgs
            .entry(user.to_string())
            .or_default()
            .push(text.to_owned());
    }
}
