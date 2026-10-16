// Import and metadata only: a full Nest app needs @nestjs/common, reflect-metadata and rxjs.
import { NestFactory, Reflector } from '@nestjs/core';

if (typeof NestFactory.create !== 'function') throw new Error('NestFactory.create');
if (typeof NestFactory.createApplicationContext !== 'function') throw new Error('createApplicationContext');
const reflector = new Reflector();
if (typeof reflector.get !== 'function') throw new Error('Reflector shape');
console.log('ok');
