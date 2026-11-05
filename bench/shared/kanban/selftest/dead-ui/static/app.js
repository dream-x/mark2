// Selftest fixture `dead-ui`: golden server, a page that renders the board
// from the API on open but never wires its controls: #add-btn, Enter and the
// card buttons do nothing. Looks finished in a screenshot.
"use strict";

const COLUMNS = ["todo", "doing", "done"];

function cardElement(card) {
  const el = document.createElement("div");
  el.className = "card";
  el.innerHTML = '<span class="title"></span><div class="actions">' +
    '<button class="move-left">←</button><button class="move-right">→</button>' +
    '<button class="delete">Delete</button></div>';
  el.querySelector(".title").textContent = card.title;
  return el;
}

fetch("/api/cards")
  .then((response) => response.json())
  .then((cards) => {
    for (const card of cards) {
      document.querySelector(`#${card.column} .cards`).append(cardElement(card));
    }
  });
