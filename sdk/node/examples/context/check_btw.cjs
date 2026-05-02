const {Agent} = require('../../index.js');

async function check() {
  try {
    process.env.KIMI_API_KEY = process.env.KIMI_API_KEY || 'dummy';
    process.env.KIMI_BASE_URL = process.env.KIMI_BASE_URL || 'http://dummy';

    const agent = await Agent.create('./agent_btw_test.acl');
    const session = agent.session('.');

    console.log('btw method exists:', typeof session.btw);
    console.log('btw is function:', typeof session.btw === 'function');

    const methods = Object.getOwnPropertyNames(Object.getPrototypeOf(session));
    console.log('Session methods:', methods.filter(m => !m.startsWith('_') && m !== 'constructor'));
  } catch (e) {
    console.error('Error:', e.message);
  }
}

check();
