// Runs the fixture's Web-standard handler on Deno.serve. Usage: deno run --allow-net=... deno.js PORT
import server from "../../../tests/fixtures/m4-acceptance/server.js";

Deno.serve({ hostname: "127.0.0.1", port: Number(Deno.args[0]), onListen() {} }, server.fetch);
