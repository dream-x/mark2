// Selftest fixture `stale-ui`: golden server, golden page except that a
// move is sent to the API but the page is not redrawn afterwards: the server
// is right, the card stays in its old column until a reload.
"use strict";

const COLUMNS = ["todo", "doing", "done"];
const input = document.getElementById("new-card");
const addBtn = document.getElementById("add-btn");

async function api(method, path, body) {
  const options = { method };
  if (body !== undefined) {
    options.headers = { "Content-Type": "application/json" };
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  if (!response.ok) {
    throw new Error(`${method} ${path}: ${response.status}`);
  }
  return response.status === 204 ? null : response.json();
}

function button(cls, label, disabled, onClick) {
  const el = document.createElement("button");
  el.type = "button";
  el.className = cls;
  el.textContent = label;
  el.disabled = disabled;
  el.addEventListener("click", onClick);
  return el;
}

function cardElement(card) {
  const index = COLUMNS.indexOf(card.column);
  const el = document.createElement("div");
  el.className = "card";
  const title = document.createElement("span");
  title.className = "title";
  title.textContent = card.title;
  const actions = document.createElement("div");
  actions.className = "actions";
  const move = (column) => api("PATCH", `/api/cards/${card.id}`, { column });  // no redraw
  actions.append(
    button("move-left", "←", index === 0, () => move(COLUMNS[index - 1])),
    button("move-right", "→", index === COLUMNS.length - 1, () => move(COLUMNS[index + 1])),
    button("delete", "Delete", false, async () => {
      await api("DELETE", `/api/cards/${card.id}`);
      await refresh();
    }),
  );
  el.append(title, actions);
  return el;
}

function render(cards) {
  for (const column of COLUMNS) {
    document.querySelector(`#${column} .cards`).replaceChildren();
  }
  for (const card of cards) {
    document.querySelector(`#${card.column} .cards`).append(cardElement(card));
  }
}

async function refresh() {
  render(await api("GET", "/api/cards"));
}

async function add() {
  const title = input.value.trim();
  if (!title) {
    return;
  }
  await api("POST", "/api/cards", { title });
  input.value = "";
  await refresh();
}

addBtn.addEventListener("click", add);
input.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    add();
  }
});

refresh();
