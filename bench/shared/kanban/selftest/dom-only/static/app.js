// Selftest fixture `dom-only`: golden server, a page that loads the board
// from the API on open and then handles add / move / delete purely in the
// DOM. On screen every click works; on the server nothing changes, and a
// reload brings the old board back.
"use strict";

const COLUMNS = ["todo", "doing", "done"];
const input = document.getElementById("new-card");

function place(el, column) {
  const index = COLUMNS.indexOf(column);
  el.dataset.column = column;
  el.querySelector(".move-left").disabled = index === 0;
  el.querySelector(".move-right").disabled = index === COLUMNS.length - 1;
  document.querySelector(`#${column} .cards`).append(el);
}

function cardElement(title, column) {
  const el = document.createElement("div");
  el.className = "card";
  el.innerHTML = '<span class="title"></span><div class="actions">' +
    '<button class="move-left">←</button><button class="move-right">→</button>' +
    '<button class="delete">Delete</button></div>';
  el.querySelector(".title").textContent = title;
  el.querySelector(".move-left").onclick = () =>
    place(el, COLUMNS[COLUMNS.indexOf(el.dataset.column) - 1]);
  el.querySelector(".move-right").onclick = () =>
    place(el, COLUMNS[COLUMNS.indexOf(el.dataset.column) + 1]);
  el.querySelector(".delete").onclick = () => el.remove();
  place(el, column);
  return el;
}

function add() {
  const title = input.value.trim();
  if (title) {
    cardElement(title, "todo");
    input.value = "";
  }
}

document.getElementById("add-btn").addEventListener("click", add);
input.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    add();
  }
});

fetch("/api/cards")
  .then((response) => response.json())
  .then((cards) => cards.forEach((card) => cardElement(card.title, card.column)));
