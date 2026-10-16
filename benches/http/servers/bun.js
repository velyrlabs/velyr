// Runs the fixture's Web-standard handler on Bun.serve. Usage: bun bun.js PORT
import server from "../../../tests/fixtures/m4-acceptance/server.js";

Bun.serve({ hostname: "127.0.0.1", port: Number(process.argv[2]), fetch: server.fetch });
