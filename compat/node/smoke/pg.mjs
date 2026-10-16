// No server: import and construct clients without connecting.
import pg from 'pg';

const { Client, Pool, escapeLiteral } = pg;
const client = new Client({ host: '127.0.0.1', port: 1, user: 'u', database: 'd' });
if (typeof client.connect !== 'function' || typeof client.query !== 'function') throw new Error('Client shape');
const pool = new Pool({ host: '127.0.0.1', port: 1, max: 1 });
if (typeof pool.connect !== 'function') throw new Error('Pool shape');
if (escapeLiteral("it's") !== "'it''s'") throw new Error('escapeLiteral');
await pool.end();
console.log('ok');
