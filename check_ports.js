// check_ports.js
const{execSync}=require('child_process');
const ns = execSync('netstat -ano', {encoding:'utf8'});

// Find all unique PIDs with LISTENING
const listening = {};
for (const line of ns.split('\n')) {
    if (line.includes('LISTENING')) {
        const parts = line.trim().split(/\s+/);
        const pid = parts[parts.length - 1];
        const addr = parts[1];
        if (!listening[pid]) listening[pid] = [];
        listening[pid].push(addr);
    }
}

// Show clash-related PIDs
const clashPids = ['9796', '6092', '49464'];
console.log('=== Clash-related LISTENING ports ===');
for (const pid of clashPids) {
    if (listening[pid]) {
        console.log(`PID ${pid}:`);
        listening[pid].forEach(a => console.log('  ' + a));
    } else {
        console.log(`PID ${pid}: NO LISTENING ports`);
    }
}

// Show all listening
console.log('\n=== All LISTENING ports ===');
for (const [pid, addrs] of Object.entries(listening)) {
    console.log(`PID ${pid}: ${addrs.join(', ')}`);
}

// Check port 9097
console.log('\n=== Connections to 9097 ===');
for (const line of ns.split('\n')) {
    if (line.includes(':9097')) {
        console.log(line.trim());
    }
}
