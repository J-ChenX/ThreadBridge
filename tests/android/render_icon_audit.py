"""Export the actual Compose paths for light/dark, 14–24dp optical review."""
from pathlib import Path
import re,os
ROOT=Path(__file__).resolve().parents[2]
SOURCE=ROOT/'android/app/src/main/java/dev/threadbridge/BridgeDesign.kt'
OUT=ROOT/('artifacts/ui-'+os.environ.get('THREADBRIDGE_QA_VERSION','0.1.19')+'-icon-audit.svg')

def path_data(body):
    result=[]
    for name,args in re.findall(r'(moveTo|lineTo|quadTo|curveTo|arcTo|close)\(([^)]*)\)',body):
        values=[v.strip().removesuffix('f') for v in args.split(',')]
        if name=='close':result.append('Z')
        elif name=='arcTo':result.append('A '+' '.join(values[:3]+['1' if v=='true' else '0' for v in values[3:5]]+values[5:]))
        else:result.append({'moveTo':'M','lineTo':'L','quadTo':'Q','curveTo':'C'}[name]+' '+' '.join(values))
    return ' '.join(result)

def main():
    source=SOURCE.read_text()
    aliases=dict(re.findall(r'private val (\w+):PathBuilder\.\(\)->Unit=\{([^\n]+)\}',source))
    icons=[]
    for line in source.splitlines():
        m=re.search(r'val (\w+)=icon\("[^\"]+"([^\n]*)',line)
        if not m:continue
        name,body=m.groups();reference=re.search(r'draw=(\w+)',body)
        commands=aliases[reference[1]] if reference else body[body.index('{')+1:]
        icons.append((name,f'<path d="{path_data(commands)}" fill="currentColor" fill-opacity="0.18"/>' if 'filled=true' in body else '',path_data(commands)))
    assert len(icons)==25,len(icons)
    # More uses native filled circles rather than the stroked path helper.
    icons.insert(5,('More',''.join(f'<circle cx="{x}" cy="12" r="1.6" fill="currentColor"/>' for x in (5,12,19)),''))
    height=65+34*len(icons)
    svg=[f'<svg xmlns="http://www.w3.org/2000/svg" width="1440" height="{height*3}" viewBox="0 0 480 {height}">']
    for panel,(bg,fg,muted) in enumerate([('#FFFFFF','#171717','#686868'),('#212121','#F0F0F0','#B5B5B5')]):
        x=panel*240
        svg.extend([f'<rect x="{x}" width="240" height="{height}" fill="{bg}"/>',f'<g fill="{fg}" font-family="sans-serif"><text x="{x+12}" y="22" font-size="12">{"Light" if panel==0 else "Dark"} · rounded 1.75</text></g>'])
        for col,size in enumerate((14,18,20,24)):
            cx=x+95+col*39
            svg.append(f'<text x="{cx}" y="46" text-anchor="middle" font-family="sans-serif" font-size="9" fill="{muted}">{size}dp</text>')
        for row,(name,fill,data) in enumerate(icons):
            cy=67+row*34
            svg.append(f'<text x="{x+12}" y="{cy+3}" font-family="sans-serif" font-size="9" fill="{muted}">{name}</text>')
            for col,size in enumerate((14,18,20,24)):
                cx=x+95+col*39
                svg.append(f'<g color="{muted if size<24 else fg}" transform="translate({cx-size/2} {cy-size/2}) scale({size/24})">{fill}<path d="{data}" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"/></g>')
    svg.append('</svg>');OUT.write_text(''.join(svg));print(OUT)
if __name__=='__main__':main()
