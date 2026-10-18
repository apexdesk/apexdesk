// Starts ApexDesk on the page (a file of its own: the server's policy
// lets the page run no inline script).
import init, { start } from "./apexdesk.js";
const loading = document.getElementById("loading");
const es = (navigator.language || "").toLowerCase().startsWith("es");
document.getElementById("loading-text").textContent = es ? "Cargando ApexDesk…" : "Loading ApexDesk…";
try {
  await init();
  await start(document.getElementById("apexdesk"));
  loading.remove();
} catch (e) {
  loading.remove();
  const err = document.getElementById("error");
  err.style.display = "block";
  err.textContent = (es ? "ApexDesk no pudo arrancar en este navegador:\n\n" : "ApexDesk could not start in this browser:\n\n") + e;
  console.error(e);
}
