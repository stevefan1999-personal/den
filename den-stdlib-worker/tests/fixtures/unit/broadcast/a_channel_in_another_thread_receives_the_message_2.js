const channel = new BroadcastChannel("across-threads");
channel.postMessage("from another thread");
channel.close();
