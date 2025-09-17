const socket = new WebSocket(WEBSOCKET_URL);

let current_option = null;
let available_options = [];
let old_username = null;

socket.addEventListener("message", (event) => {
  {
    const data = JSON.parse(event.data);

    switch (data["key"]) {
      case "UpdatePage":
        console.log("replacing main");
        document.getElementById("main").outerHTML = data["main_contents"];
        update_last_refreshed();
        break;
      case "SetUsername":
        set_username(data["new_name"]);
        break;
      case "UsernameSuggestions":
        apply_username_suggestions(data["suggestions"]);
        break;
      default:
        console.log("unknown message:", data);
        break;
    }
  }
});

function update_last_refreshed() {
  const d = new Date();
  const n = d.toLocaleTimeString();
  document.getElementById("refresh-time").innerText = `last refresh: ${n}`;
}

function set_username(name) {
  close_popup(false);

  // new data is coming
  document.getElementById("main").innerHTML = "";
  document.getElementById("refresh-time").innerText =
    `getting PR data for user ${name}`;

  document.getElementById("change-username-input").value = name;
}

function select_username(name) {
  if (name === old_username) {
    return;
  }

  // new data is coming
  document.getElementById("main").innerHTML = "";
  document.getElementById("refresh-time").innerText =
    `getting PR data for user ${name}`;

  close_popup(false);

  socket.send(
    JSON.stringify({
      key: "UsernameSelect",
      selected_name: name,
    }),
  );
}

function changeSelection(f) {
  if (available_options.length === 0) {
    return;
  }

  for (const i of available_options) {
    i.ariaSelected = "false";
  }

  f();

  if (current_option >= available_options.length) {
    current_option -= available_options.length;
  } else if (current_option < 0) {
    current_option += available_options.length;
  }

  available_options[current_option].ariaSelected = "true";
}

function selectNext() {
  changeSelection(() => {
    if (current_option === null) {
      current_option = 0;
    } else {
      current_option += 1;
    }
  });
}

function selectPrev() {
  changeSelection(() => {
    if (current_option === null) {
      current_option = available_options.length - 1;
    } else {
      current_option -= 1;
    }
  });
}

function confirmSelect(enter_pressed = false) {
  const popup = document.getElementById("suggestions-popup");
  const input_elem = document.getElementById("change-username-input");
  input_elem.blur();

  if (enter_pressed) {
    console.log("force-select");
    const input = document.getElementById("change-username-input");
    select_username(input.value);
  } else if (current_option === null) {
    close_popup(false);
  } else {
    available_options[current_option].click();
  }
}

function close_popup(reset) {
  if (reset && old_username !== null) {
    const input_elem = document.getElementById("change-username-input");
    input_elem.value = old_username;
  }

  console.log("reset old username");

  old_username = null;
  const popup = document.getElementById("suggestions-popup");
  popup.style.display = "none";
  popup.innerHTML = "";
}

function apply_username_suggestions(suggestions) {
  if (suggestions.length === 0) {
    return;
  }

  current_option = null;
  available_options = [];

  const popup = document.getElementById("suggestions-popup");
  popup.style.display = "flex";
  popup.innerHTML = "";

  for (const i of suggestions) {
    const elem = document.createElement("li");
    available_options.push(elem);

    elem.role = "option";
    elem.ariaSelected = "false";
    elem.classList.add("username-suggestion");
    elem.classList.add("author");

    const image = document.createElement("img");
    image.classList.add("avatar");
    image.src = i["avatar_url"];

    const name = document.createElement("span");
    name.innerText = i["name"];

    elem.appendChild(image);
    elem.appendChild(name);
    popup.appendChild(elem);

    elem.addEventListener("click", () => select_username(i["name"]));
  }
}

function ask_suggestions(event) {
  socket.send(
    JSON.stringify({
      key: "UsernameSuggestions",
      current_value: event.target.value,
    }),
  );
}

addEventListener("DOMContentLoaded", (event) => {
  const form = document.getElementById("change-username");
  const input_elem = document.getElementById("change-username-input");
  const reset_button = document.getElementById("change-username-reset");

  form.addEventListener("submit", (event) => {
    event.preventDefault();
  });

  reset_button.addEventListener("click", () => {
    const popup = document.getElementById("suggestions-popup");
    close_popup(false);

    // new data is coming
    document.getElementById("main").innerHTML = "";
    document.getElementById("refresh-time").innerText = `getting your PR data`;
    socket.send(JSON.stringify({ key: "ResetUsername" }));
  });

  input_elem.addEventListener("click", (event) => {
    if (old_username === null) {
      console.log("set old username");
      const input_elem = document.getElementById("change-username-input");
      old_username = input_elem.value;
    }

    ask_suggestions(event);
  });

  input_elem.addEventListener("keydown", (event) => {
    if (old_username === null) {
      console.log("set old username");
      const input_elem = document.getElementById("change-username-input");
      old_username = input_elem.value;
    }

    switch (event.key) {
      case "ArrowDown":
        selectNext();
        event.stopImmediatePropagation();
        break;
      case "ArrowUp":
        selectPrev();
        event.stopImmediatePropagation();
        break;
      case "Tab":
        if (event.shiftKey) {
          selectPrev();
        } else {
          selectNext();
        }
        event.stopImmediatePropagation();
        event.preventDefault();
        break;
      case "Enter":
        confirmSelect(true);
        break;
      case "Esc":
        close_popup(true);
        break;
    }
  });

  document.addEventListener("keydown", (event) => {
    switch (event.key) {
      case "/":
        const input_elem = document.getElementById("change-username-input");
        if (input_elem !== document.activeElement) {
          input_elem.focus();
          input_elem.select();
          event.stopImmediatePropagation();
          event.preventDefault();
        }
        break;
    }
  });

  document.addEventListener("click", (event) => {
    let target = event.target;
    let ctr = 0;

    while (ctr < 10) {
      if (target === null) {
        break;
      }
      if (
        target.id === "change-username" ||
        (target.classList && target.classList.contains("username-suggestion"))
      ) {
        return;
      }

      ctr += 1;
      target = target.parentNode;
    }

    close_popup(true);
  });

  input_elem.addEventListener("input", (event) => {
    ask_suggestions(event);
  });
});
