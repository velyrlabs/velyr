// No server: import and construct a lazy pool without connecting.
import mysql from 'mysql2';

const pool = mysql.createPool({ host: '127.0.0.1', port: 1, user: 'u', database: 'd', connectionLimit: 1 });
if (typeof pool.query !== 'function' || typeof pool.getConnection !== 'function') throw new Error('Pool shape');
if (mysql.format('select ? as a, ? as b', [1, "x'y"]) !== "select 1 as a, 'x\\'y' as b") throw new Error('format');
if (mysql.escapeId('a`b') !== '`a``b`') throw new Error('escapeId');
await new Promise((resolve) => pool.end(resolve));
console.log('ok');
