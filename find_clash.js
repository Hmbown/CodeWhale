// find_clash.js
const{execSync}=require('child_process');
const fs=require('fs');
const p=require('path');
const h=require('os').homedir();

console.log('=== Clash Ninja Diagnostic ===\n');

// 1. Find executable path via PowerShell
try {
    const cmd = 'powershell -Command "Get-Process clash-ninja | Select-Object Id,Path,MainWindowTitle | Format-List"';
    const out = execSync(cmd, {encoding:'utf8', timeout:15000});
    console.log('[Process Info]');
    console.log(out);
} catch(e) {
    console.log('Process info err:', e.message);
}

// 2. Search for Clash Ninja config locations
const searchPaths = [
    p.join(h, '.config', 'clash-ninja'),
    p.join(h, '.config', 'clash'),
    p.join(h, '.config', 'clash-verge'),
    p.join(h, 'AppData', 'Roaming', 'clash-ninja'),
    p.join(h, 'AppData', 'Roaming', 'clash-verge-rev'),
    p.join(h, 'AppData', 'Roaming', 'io.github.clash-verge-rev'),
    p.join(h, 'AppData', 'Local', 'clash-ninja'),
    p.join(h, 'AppData', 'Local', 'Programs', 'clash-ninja'),
];

console.log('\n[Config Search]');
let found = false;
for (const sp of searchPaths) {
    if (fs.existsSync(sp)) {
        found = true;
        console.log('FOUND:', sp);
        try {
            const files = fs.readdirSync(sp);
            files.forEach(f => console.log('  ', f));
        } catch(e) {}
    }
}
if (!found) console.log('No standard config path found');

// 3. Check netstat for PID 9796
console.log('\n[Listening Ports]');
try {
    const ns = execSync('netstat -ano', {encoding:'utf8', timeout:10000});
    const lines = ns.split('\n').filter(l => l.includes('9796') && l.includes('LISTENING'));
    if (lines.length === 0) {
        console.log('No LISTENING ports for PID 9796');
    } else {
        lines.forEach(l => console.log(l));
    }
} catch(e) {
    console.log('netstat err:', e.message);
}

// 4. Test common proxy ports
console.log('\n[Proxy Port Test]');
for (const port of [7890, 7891, 7892, 7893, 9090]) {
    try {
        const r = execSync(`curl --connect-timeout 2 http://127.0.0.1:${port} 2>NUL`, {encoding:'utf8', timeout:5000});
        console.log(`Port ${port}: reachable`);
    } catch(e) {
        // silent
    }
}

// 5. System proxy
console.log('\n[System Proxy]');
try {
    const ps = 'powershell -Command "Get-ItemProperty -Path \'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\' | Select-Object ProxyEnable,ProxyServer | Format-List"';
    const out = execSync(ps, {encoding:'utf8', timeout:10000});
    console.log(out);
} catch(e) {
    console.log('err:', e.message);
}

console.log('=== Done ===');
