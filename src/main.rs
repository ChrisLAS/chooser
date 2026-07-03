use std::{
    collections::BTreeSet,
    io::{self, IsTerminal, Write},
    time::Duration,
};

use anyhow::{anyhow, Context};
use clap::Parser;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    },
    DefaultTerminal, Frame,
};
use tailtalk::{adsp::AdspAddress, nbp::RegisteredName, TalkStack};
use tailtalk_packets::{
    aarp::AppleTalkAddress,
    nbp::{EntityName, NbpTuple},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    time::{self, MissedTickBehavior},
};

#[derive(Parser, Debug)]
#[command(about = "Browse AppleTalk NBP services with TailTalk")]
struct Args {
    /// Network interface to bind to for EtherTalk, such as eth0 or enp3s0
    #[arg(short, long)]
    interface: Option<String>,

    /// TashTalk serial port path for LocalTalk, such as /dev/ttyUSB0
    #[arg(short, long)]
    tashtalk: Option<String>,

    /// NBP query in Object:Type@Zone format
    #[arg(short, long, default_value = "=:=@*")]
    entity: String,

    /// Refresh interval in seconds
    #[arg(short = 'r', long, default_value_t = 2)]
    refresh: u64,

    /// Print a simple table instead of launching the TUI
    #[arg(long)]
    plain: bool,

    /// Plain mode: print each refresh below the previous one instead of clearing
    #[arg(long)]
    no_clear: bool,

    /// Name to advertise as an AirTalk peer
    #[arg(long)]
    name: Option<String>,

    /// Disable NBP peer advertisement and ADSP message listener
    #[arg(long)]
    no_advertise: bool,

    /// NBP service type used for Linux-to-Linux AirTalk peers
    #[arg(long, default_value = "AirTalk")]
    peer_type: String,

    /// Message sent to the selected AirTalk peer with Enter
    #[arg(long)]
    message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Service {
    name: String,
    kind: String,
    zone: String,
    network_number: u16,
    node_id: u8,
    socket_number: u8,
    socket: String,
}

struct App {
    entity: EntityName,
    local_name: String,
    peer_type: String,
    message: String,
    rows: Vec<Service>,
    filter: String,
    status: String,
    refresh_count: u64,
    editing_filter: bool,
    selected_type: usize,
    selected_row: usize,
}

enum InputAction {
    None,
    Quit,
    Refresh,
    Ping,
    Message,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let entity: EntityName = args
        .entity
        .as_str()
        .try_into()
        .map_err(|err| anyhow!("invalid NBP entity '{}': {}", args.entity, err))?;
    let stack = build_stack(&args).await?;
    let peer = setup_peer(&stack, &args).await?;

    if args.plain {
        run_plain(stack, entity, &args, peer.incoming).await
    } else {
        run_tui(stack, entity, peer, &args).await
    }
}

async fn build_stack(args: &Args) -> anyhow::Result<TalkStack> {
    if args.interface.is_none() && args.tashtalk.is_none() {
        anyhow::bail!("at least one of --interface or --tashtalk is required");
    }

    let mut builder = TalkStack::builder();
    if let Some(interface) = &args.interface {
        builder = builder.ethernet(interface);
    }
    if let Some(tashtalk) = &args.tashtalk {
        builder = builder.localtalk(tashtalk);
    }
    builder
        .build()
        .await
        .context("failed to build TailTalk AppleTalk stack")
}

struct PeerRuntime {
    local_name: String,
    peer_type: String,
    message: String,
    incoming: Option<mpsc::UnboundedReceiver<String>>,
}

async fn setup_peer(stack: &TalkStack, args: &Args) -> anyhow::Result<PeerRuntime> {
    let local_name = sanitize_nbp_part(&args.name.clone().unwrap_or_else(default_peer_name));
    let message = args
        .message
        .clone()
        .unwrap_or_else(|| format!("Hello from {local_name}"));

    if args.no_advertise {
        return Ok(PeerRuntime {
            local_name,
            peer_type: args.peer_type.clone(),
            message,
            incoming: None,
        });
    }

    let (socket, mut listener) = stack
        .listen_adsp(None)
        .await
        .context("failed to start AirTalk ADSP listener")?;
    let entity: EntityName = format!("{}:{}@*", local_name, args.peer_type)
        .as_str()
        .try_into()
        .map_err(|err| anyhow!("invalid advertised peer name: {err}"))?;
    stack
        .nbp
        .register(RegisteredName {
            name: entity,
            sock_num: socket,
        })
        .await
        .context("failed to register AirTalk NBP name")?;

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok(mut stream) = listener.accept().await else {
                break;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let remote = stream.remote_addr();
                let mut data = Vec::new();
                let status = match stream.read_to_end(&mut data).await {
                    Ok(_) => format!(
                        "message from {}.{}:{}: {}",
                        remote.network_number,
                        remote.node_number,
                        remote.socket_number,
                        String::from_utf8_lossy(&data)
                    ),
                    Err(err) => format!("failed reading ADSP message: {err}"),
                };
                let _ = tx.send(status);
            });
        }
    });

    Ok(PeerRuntime {
        local_name,
        peer_type: args.peer_type.clone(),
        message,
        incoming: Some(rx),
    })
}

fn default_peer_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "chooser-linux".into())
}

fn sanitize_nbp_part(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| match c {
            ':' | '@' | '*' | '=' | '≈' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches('-').trim();
    if cleaned.is_empty() {
        "chooser-linux".into()
    } else {
        cleaned.chars().take(31).collect()
    }
}

async fn run_tui(
    stack: TalkStack,
    entity: EntityName,
    mut peer: PeerRuntime,
    args: &Args,
) -> anyhow::Result<()> {
    let mut terminal = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    let mut interval = time::interval(Duration::from_secs(args.refresh.max(1)));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut app = App::new(entity, &peer);

    loop {
        terminal.draw(|frame| draw(frame, &mut app))?;
        tokio::select! {
            maybe_event = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    match handle_key(&mut app, key) {
                        InputAction::Quit => break,
                        InputAction::Refresh => refresh(&stack, &mut app).await,
                        InputAction::Ping => ping_selected(&stack, &mut app).await,
                        InputAction::Message => message_selected(&stack, &mut app).await,
                        InputAction::None => {}
                    }
                }
            }
            result = tokio::signal::ctrl_c() => {
                result.context("failed to listen for Ctrl-C")?;
                break;
            }
            _ = interval.tick() => refresh(&stack, &mut app).await,
            message = recv_peer_message(&mut peer.incoming) => {
                if let Some(message) = message {
                    app.status = message;
                }
            }
        }
    }

    Ok(())
}

async fn recv_peer_message(rx: &mut Option<mpsc::UnboundedReceiver<String>>) -> Option<String> {
    if let Some(rx) = rx {
        rx.recv().await
    } else {
        std::future::pending().await
    }
}

async fn refresh(stack: &TalkStack, app: &mut App) {
    app.refresh_count += 1;
    match stack.nbp.lookup(app.entity.clone()).await {
        Ok(tuples) => {
            app.rows = services(&tuples);
            app.status = format!("{} services visible", app.rows.len());
            app.clamp_selection();
        }
        Err(err) => app.status = format!("lookup failed: {err}"),
    }
}

async fn ping_selected(stack: &TalkStack, app: &mut App) {
    let Some(row) = app.selected_service() else {
        app.status = "no service selected".into();
        return;
    };
    let addr = AppleTalkAddress {
        network_number: row.network_number,
        node_number: row.node_id,
    };
    match stack.echo.send(addr, b"chooser").await {
        Ok(rtt) => app.status = format!("AEP echo from {} in {} ms", row.name, rtt.as_millis()),
        Err(err) => app.status = format!("AEP echo to {} failed: {err}", row.name),
    }
}

async fn message_selected(stack: &TalkStack, app: &mut App) {
    let Some(row) = app.selected_service() else {
        app.status = "no AirTalk peer selected".into();
        return;
    };
    if row.kind != app.peer_type {
        app.status = format!("select a {} peer before sending a message", app.peer_type);
        return;
    }

    let addr = AdspAddress {
        network_number: row.network_number,
        node_number: row.node_id,
        socket_number: row.socket_number,
    };
    match stack.connect_adsp(addr).await {
        Ok(mut stream) => {
            let result = async {
                stream.write_all(app.message.as_bytes()).await?;
                stream.write_eom().await?;
                stream.close().await
            }
            .await;
            match result {
                Ok(()) => app.status = format!("sent ADSP message to {}", row.name),
                Err(err) => app.status = format!("ADSP send to {} failed: {err}", row.name),
            }
        }
        Err(err) => app.status = format!("ADSP connect to {} failed: {err}", row.name),
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> InputAction {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return InputAction::Quit;
    }
    if app.editing_filter {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.editing_filter = false,
            KeyCode::Backspace => {
                app.filter.pop();
                app.clamp_selection();
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
                app.clamp_selection();
            }
            _ => {}
        }
        return InputAction::None;
    }

    match key.code {
        KeyCode::Char('q') => InputAction::Quit,
        KeyCode::Char('/') => {
            app.editing_filter = true;
            InputAction::None
        }
        KeyCode::Char('r') => InputAction::Refresh,
        KeyCode::Char('p') => InputAction::Ping,
        KeyCode::Enter => InputAction::Message,
        KeyCode::Esc => {
            app.filter.clear();
            app.clamp_selection();
            InputAction::None
        }
        KeyCode::Up => {
            app.selected_row = app.selected_row.saturating_sub(1);
            InputAction::None
        }
        KeyCode::Down => {
            app.selected_row += 1;
            app.clamp_selection();
            InputAction::None
        }
        KeyCode::Left => {
            app.selected_type = app.selected_type.saturating_sub(1);
            app.selected_row = 0;
            InputAction::None
        }
        KeyCode::Right => {
            app.selected_type += 1;
            app.selected_row = 0;
            app.clamp_selection();
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(5),
        ])
        .split(area);
    draw_header(frame, app, vertical[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(44)])
        .split(vertical[1]);
    draw_types(frame, app, body[0]);
    draw_table(frame, app, body[1]);
    draw_details(frame, app, vertical[2]);
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let filter = if app.filter.is_empty() {
        "none".into()
    } else {
        app.filter.clone()
    };
    let title = Line::from(vec![
        Span::styled(
            "Chooser",
            Style::default().fg(Color::Black).bg(Color::White),
        ),
        Span::raw(format!(
            "  NBP {}  refresh {}  filter: {}",
            app.entity, app.refresh_count, filter
        )),
        Span::raw(format!("  local: {}:{}", app.local_name, app.peer_type)),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::ALL).title("AirTalk")),
        area,
    );
}

fn draw_types(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let types = app.service_types();
    let items: Vec<_> = types
        .iter()
        .map(|kind| ListItem::new(kind.as_str()))
        .collect();
    let mut state = ListState::default().with_selected(Some(app.selected_type));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title("AppleTalk"))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

fn draw_table(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let rows = app.visible_rows();
    let table_rows = rows.iter().map(|row| {
        Row::new([
            row.name.as_str(),
            row.kind.as_str(),
            row.zone.as_str(),
            row.socket.as_str(),
        ])
    });
    let mut state = TableState::default().with_selected(Some(app.selected_row));
    frame.render_stateful_widget(
        Table::new(
            table_rows,
            [
                Constraint::Percentage(38),
                Constraint::Percentage(25),
                Constraint::Percentage(20),
                Constraint::Percentage(17),
            ],
        )
        .header(
            Row::new(["Name", "Type", "Zone", "Socket"]).style(Style::default().fg(Color::Yellow)),
        )
        .block(Block::default().borders(Borders::ALL).title("Services"))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

fn draw_details(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let selected = app.selected_service();
    let detail = selected.map_or_else(
        || "No service selected".into(),
        |row| {
            format!(
                "{}:{}@{}  socket {}",
                row.name, row.kind, row.zone, row.socket
            )
        },
    );
    let mode = if app.editing_filter {
        "typing filter; Enter/Esc finishes"
    } else {
        "arrows select  / filter  Esc clear  r refresh  p ping  Enter message  q quit"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(app.status.as_str()),
            Line::from(detail),
            Line::from(mode),
        ])
        .block(Block::default().borders(Borders::ALL).title("Status")),
        area,
    );
}

async fn run_plain(
    stack: TalkStack,
    entity: EntityName,
    args: &Args,
    mut incoming: Option<mpsc::UnboundedReceiver<String>>,
) -> anyhow::Result<()> {
    let clear_screen = !args.no_clear && io::stdout().is_terminal();
    let mut interval = time::interval(Duration::from_secs(args.refresh.max(1)));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut refresh_count = 0_u64;

    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("failed to listen for Ctrl-C")?;
                println!("\nchooser: exiting");
                break;
            }
            _ = interval.tick() => {
                refresh_count += 1;
                let lookup = stack.nbp.lookup(entity.clone()).await;
                render_plain(refresh_count, &entity, lookup.as_deref().ok(), lookup.as_ref().err(), clear_screen)?;
            }
            message = recv_peer_message(&mut incoming) => {
                if let Some(message) = message {
                    eprintln!("{message}");
                }
            }
        }
    }
    Ok(())
}

fn render_plain(
    refresh_count: u64,
    entity: &EntityName,
    tuples: Option<&[NbpTuple]>,
    error: Option<&io::Error>,
    clear_screen: bool,
) -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    if clear_screen {
        write!(stdout, "\x1b[2J\x1b[H")?;
    } else {
        writeln!(stdout, "\n--- refresh {refresh_count} ---")?;
    }
    writeln!(stdout, "chooser - NBP services for {entity}")?;
    writeln!(stdout, "Refresh: {refresh_count}    Ctrl-C exits")?;
    if let Some(error) = error {
        writeln!(stdout, "\nLookup failed: {error}")?;
    } else {
        writeln!(stdout, "\n{}", plain_table(tuples.unwrap_or_default()))?;
    }
    stdout.flush()?;
    Ok(())
}

fn plain_table(tuples: &[NbpTuple]) -> String {
    let rows = services(tuples);
    let mut out = format!(
        "{:<30.30}  {:<18.18}  {:<16.16}  {}\n{:-<30}  {:-<18}  {:-<16}  {:-<12}\n",
        "Name", "Type", "Zone", "Socket", "", "", "", ""
    );
    if rows.is_empty() {
        out.push_str("(no services found)\n");
        return out;
    }
    for row in rows {
        out.push_str(&format!(
            "{:<30.30}  {:<18.18}  {:<16.16}  {}\n",
            row.name, row.kind, row.zone, row.socket
        ));
    }
    out
}

fn services(tuples: &[NbpTuple]) -> Vec<Service> {
    let mut rows: Vec<_> = tuples
        .iter()
        .map(|tuple| Service {
            name: tuple.entity_name.object.clone(),
            kind: tuple.entity_name.entity_type.clone(),
            zone: tuple.entity_name.zone.clone(),
            network_number: tuple.network_number,
            node_id: tuple.node_id,
            socket_number: tuple.socket_number,
            socket: format!(
                "{}.{}:{}",
                tuple.network_number, tuple.node_id, tuple.socket_number
            ),
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

impl App {
    fn new(entity: EntityName, peer: &PeerRuntime) -> Self {
        Self {
            entity,
            local_name: peer.local_name.clone(),
            peer_type: peer.peer_type.clone(),
            message: peer.message.clone(),
            rows: Vec::new(),
            filter: String::new(),
            status: "waiting for first lookup".into(),
            refresh_count: 0,
            editing_filter: false,
            selected_type: 0,
            selected_row: 0,
        }
    }

    fn service_types(&self) -> Vec<String> {
        let mut types = BTreeSet::from(["All".to_string()]);
        types.extend(self.filtered_rows().into_iter().map(|row| row.kind.clone()));
        types.into_iter().collect()
    }

    fn visible_rows(&self) -> Vec<Service> {
        let types = self.service_types();
        let selected = types
            .get(self.selected_type)
            .map(String::as_str)
            .unwrap_or("All");
        self.filtered_rows()
            .into_iter()
            .filter(|row| selected == "All" || row.kind == selected)
            .collect()
    }

    fn filtered_rows(&self) -> Vec<Service> {
        let filter = self.filter.to_lowercase();
        self.rows
            .iter()
            .filter(|row| {
                filter.is_empty()
                    || row.name.to_lowercase().contains(&filter)
                    || row.kind.to_lowercase().contains(&filter)
                    || row.zone.to_lowercase().contains(&filter)
                    || row.socket.contains(&filter)
            })
            .cloned()
            .collect()
    }

    fn selected_service(&self) -> Option<Service> {
        self.visible_rows().get(self.selected_row).cloned()
    }

    fn clamp_selection(&mut self) {
        let type_len = self.service_types().len().max(1);
        self.selected_type = self.selected_type.min(type_len - 1);
        let row_len = self.visible_rows().len().max(1);
        self.selected_row = self.selected_row.min(row_len - 1);
    }
}

struct TerminalGuard {
    terminal: DefaultTerminal,
}

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self {
            terminal: ratatui::init(),
        })
    }

    fn draw(&mut self, f: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        self.terminal.draw(f).map(|_| ())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        ratatui::restore();
    }
}
