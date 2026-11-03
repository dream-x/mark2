const WS_URL = `${location.protocol === "https:" ? "wss://" : "ws://"}${location.host}/ws`;

let ws = null;
let joined = false;
let nick = null;
let historyMessages = [];

function addMessage(type, text, from, ts) {
  const el = document.getElementById("messages");
  const div = document.createElement("div");

  if (type === "system") {
    div.className = "message system";
    div.textContent = text;
  } else {
    div.className = `message ${from === nick ? "own" : "other"}`;
    const nickEl = document.createElement("span");
    nickEl.className = "nick";
    nickEl.textContent = `${from}: `;
    const textEl = document.createElement("span");
    textEl.textContent = text;
    const tsEl = document.createElement("span");
    tsEl.className = "ts";
    tsEl.textContent = ts ? new Date(ts).toLocaleTimeString() : "";
    div.appendChild(nickEl);
    div.appendChild(textEl);
    if (tsEl.textContent) div.appendChild(tsEl);
  }

  el.appendChild(div);
  el.scrollTop = el.scrollHeight;
}

function handleMessage(data) {
  if (data.type === "history") {
    historyMessages = data.messages || [];
    const el = document.getElementById("messages");
    el.innerHTML = "";
    for (const m of historyMessages) {
      addMessage(m.type, m.text, m.nick, m.ts);
    }
  } else if (data.type === "system") {
    addMessage("system", data.text || "", null, null);
  } else if (data.type === "message") {
    addMessage("message", data.text || "", data.nick, data.ts);
  }
}

function doJoin(nickValue) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: "join", nick: nickValue }));
  }
  nick = nickValue;
  joined = true;
  const nickInput = document.getElementById("nick");
  nickInput.disabled = true;
  document.getElementById("join-btn").disabled = true;
  const msgArea = document.getElementById("msg-area");
  msgArea.classList.add("visible");
  const msgInput = document.getElementById("msg");
  msgInput.disabled = false;
  msgInput.focus();
}

function connect() {
  ws = new WebSocket(WS_URL);
  ws.onopen = () => {
    if (joined && nick) {
      ws.send(JSON.stringify({ type: "join", nick }));
    }
  };
  ws.onmessage = (event) => {
    try {
      handleMessage(JSON.parse(event.data));
    } catch (e) {}
  };
  ws.onclose = () => {
    setTimeout(connect, 1000);
  };
}

function sendMessage() {
  const text = document.getElementById("msg").value.trim();
  if (text && ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: "message", text }));
    document.getElementById("msg").value = "";
    document.getElementById("msg").focus();
  }
}

document.getElementById("join-btn").addEventListener("click", () => {
  const nickInput = document.getElementById("nick");
  const nick = nickInput.value.trim() || "anon";
  if (!joined) {
    doJoin(nick);
  }
});

document.getElementById("nick").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    if (!joined) {
      document.getElementById("join-btn").click();
    }
  }
});

document.getElementById("msg").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    sendMessage();
  }
});

document.getElementById("send-btn").addEventListener("click", () => {
  sendMessage();
});

window.addEventListener("load", () => {
  connect();
});