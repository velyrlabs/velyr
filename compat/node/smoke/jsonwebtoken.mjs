import jwt from 'jsonwebtoken';

const token = jwt.sign({ sub: 'u1' }, 'secret', { algorithm: 'HS256', expiresIn: '1h' });
const claims = jwt.verify(token, 'secret');
if (claims.sub !== 'u1' || typeof claims.exp !== 'number') throw new Error(JSON.stringify(claims));
let rejected = false;
try { jwt.verify(token, 'wrong'); } catch (err) { rejected = err.name === 'JsonWebTokenError'; }
if (!rejected) throw new Error('wrong secret was accepted');
if (jwt.decode(token).sub !== 'u1') throw new Error('decode');
console.log('ok');
