const list = document.getElementById("files");
const message = document.getElementById("message");

function say(text) {
  message.textContent = text;
}

async function loadFiles() {
  const response = await fetch("/uploads/");
  const html = await response.text();
  const doc = new DOMParser().parseFromString(html, "text/html");
  const links = Array.from(doc.querySelectorAll("a"))
    .map((a) => a.getAttribute("href"))
    .filter((href) => href && !href.endsWith("/") && href !== ".gitkeep");

  list.replaceChildren();
  if (links.length === 0) {
    const item = document.createElement("li");
    item.textContent = "No uploaded files yet.";
    list.append(item);
    return;
  }
  for (const href of links) {
    const item = document.createElement("li");
    const link = document.createElement("a");
    link.href = "/uploads/" + href;
    link.textContent = decodeURIComponent(href);
    const button = document.createElement("button");
    button.textContent = "Delete";
    button.addEventListener("click", () => deleteFile(href));
    item.append(link, " ", button);
    list.append(item);
  }
}

async function deleteFile(href) {
  const response = await fetch("/uploads/" + href, { method: "DELETE" });
  say("DELETE /uploads/" + decodeURIComponent(href) + " -> " + response.status + " " + response.statusText);
  await loadFiles();
}

loadFiles().catch((error) => say("Could not load the file list: " + error));
