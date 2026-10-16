const channel = new BroadcastChannel("m5");
channel.postMessage("from worker");
channel.close();
