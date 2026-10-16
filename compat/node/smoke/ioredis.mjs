// No server: import and construct a lazy client without connecting.
import Redis from 'ioredis';

const redis = new Redis({ host: '127.0.0.1', port: 1, lazyConnect: true, retryStrategy: () => null });
if (typeof redis.get !== 'function' || typeof redis.pipeline !== 'function') throw new Error('Redis shape');
if (redis.status !== 'wait') throw new Error(`unexpected status ${redis.status}`);
redis.disconnect();
console.log('ok');
