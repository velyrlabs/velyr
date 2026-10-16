// bcryptjs is the pure JavaScript variant; nothing native.
import bcrypt from 'bcryptjs';

const hash = bcrypt.hashSync('password', 4);
if (!hash.startsWith('$2')) throw new Error(hash);
if (!bcrypt.compareSync('password', hash)) throw new Error('compareSync');
if (bcrypt.compareSync('other', hash)) throw new Error('wrong password accepted');
const asyncHash = await bcrypt.hash('pw', 4);
if (!(await bcrypt.compare('pw', asyncHash))) throw new Error('async compare');
console.log('ok');
