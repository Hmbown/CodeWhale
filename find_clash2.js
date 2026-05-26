// find_clash2.js
const{execSync}=require('child_process');
const fs=require('fs');
const p=require('path');

const clashDir = 'C:\\Program Files\\Clash V-Ninja';
console.log('=== Clash V-Ninja Directory ===');
if (fs.existsSync(clashDir)) {
    function walk(dir, indent) {
        try {
            const entries = fs.readdirSync(dir, {withFileTypes:true});
            for (const e of entries) {
                const full = p.join(dir, e.name);
                if (e.isDirectory()) {
                    console.log(indent + '[DIR]', e.name);
                    walk(full, indent + '  ');
                } else if (e.name.endsWith('.yaml') || e.name.endsWith('.yml') || e.name.endsWith('.json') || e.name.endsWith('.exe') || e.name.endsWith('.dll') || e.name.endsWith('.txt')) {
                    try {
                        const stat = fs.statSync(full);
                        console.log(indent + e.name, '(' + (stat.size/1024).toFixed(1) + ' KB)');
                    } catch(ex) {}
                } else {
                    try {
                        const stat = fs.statSync(full);
                        console.log(indent + e.name, '(' + (stat.size/1024).toFixed(1) + ' KB)');
                    } catch(ex) {}
                }
            }
        } catch(e) {
            console.log(indent + 'ERROR:', e.message);
        }
    }
    walk(clashDir, '');
} else {
    console.log('Directory not found');
}

// Check service PID ports
console.log('\n=== Service PID 6092 ports ===');
try {
    const ns = execSync('netstat -ano', {encoding:'utf8'});
    const lines = ns.split('\n').filter(l => l.includes('6092'));
    lines.forEach(l => console.log(l.trim()));
    if (lines.length === 0) console.log('No connections for PID 6092');
} catch(e) {
    console.log('err:', e.message);
}

// Check all clash-ninja ports
console.log('\n=== All clash-ninja ports ===');
try {
    const ns = execSync('netstat -ano', {encoding:'utf8'});
    const lines = ns.split('\n').filter(l => l.includes('9796') || l.includes('6092'));
    lines.forEach(l => console.log(l.trim()));
} catch(e) {
    console.log('err:', e.message);
}
