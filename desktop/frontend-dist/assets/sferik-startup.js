(()=>{
 const host=document.getElementById('sferik-startup');if(!host)return;
 const canvas=host.querySelector('canvas'),ctx=canvas.getContext('2d'),hero=host.querySelector('.ss-hero'),eyes=host.querySelector('.ss-eyes'),body=host.querySelector('.ss-shell').parentElement,shadow=host.querySelector('.ss-shadow'),halo=host.querySelector('.ss-halo'),label=host.querySelector('.ss-label');
 const reduced=matchMedia('(prefers-reduced-motion: reduce)').matches||(()=>{try{return JSON.parse(localStorage.getItem('sfery-sferik-settings')||'{}').reduceMotion===true}catch{return false}})();
 const darkTheme=localStorage.getItem('sfery-theme')==='dark';
 host.dataset.dark=String(darkTheme);
 const palette=darkTheme?{core:'225,248,255',trail:'219,248,255',glow:'#b5eaff',ring:'142,222,255'}:{core:'24,61,89',trail:'39,78,108',glow:'rgba(38,77,110,.35)',ring:'39,78,108'};
 let width=0,height=0,dpr=1,done=false,target=null,targetOpacity='',start=performance.now();
 const clamp=(x,a=0,b=1)=>Math.max(a,Math.min(b,x)),ease=x=>1-(1-clamp(x))**3,smooth=x=>{x=clamp(x);return x*x*(3-2*x)};
 const rand=i=>{let x=Math.sin(i*127.1+42.3)*43758.5453;return x-Math.floor(x)};
 const launchAt=1.05,launchSpread=3.35,wakeAt=launchAt+launchSpread+.86+.03,travelAt=wakeAt+.45,endAt=travelAt+.8;
 // Stratified positions keep equal density across the entire viewport. Slight
 // jitter avoids a visible grid, without clustering the source above the hero.
 let particles=[];
 function size(){dpr=Math.min(devicePixelRatio||1,2);width=innerWidth;height=innerHeight;canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);ctx.setTransform(dpr,0,0,dpr,0,0);
  const columns=Math.max(10,Math.round(width/82)),rows=Math.max(7,Math.round(height/82));
  particles=Array.from({length:columns*rows},(_,i)=>({id:i,x:((i%columns)+.12+rand(i+11)*.76)/columns,y:(Math.floor(i/columns)+.12+rand(i+29)*.76)/rows,size:.7+rand(i+31)**2*5.8,duration:.68+rand(i+83)*.18,birth:rand(i+53)*.3,curve:(rand(i+90)-.5)*24}));
  // A shuffled launch order spreads the stream over time and across the screen.
  // Each spark starts individually; eyes wake only after the last arrival.
  [...particles].sort((a,b)=>rand(a.id+70)-rand(b.id+70)).forEach((p,rank)=>{p.delay=rank/Math.max(1,particles.length-1)*launchSpread});
 }
 size();addEventListener('resize',size,{passive:true});
 function finish(){if(done)return;done=true;if(target)target.style.opacity=targetOpacity;host.remove();removeEventListener('resize',size)}
 setTimeout(finish,(endAt+2)*1000);
 host.querySelector('.ss-skip').addEventListener('click',finish);
 if(reduced){hero.style.transform='translate(-50%,-50%) scale(2.5)';eyes.style.transform='scaleY(1)';eyes.style.opacity='1';setTimeout(finish,350);return}
 function frame(now){if(done)return;const t=(now-start)/1000,cx=width/2,cy=height*.46;
  ctx.clearRect(0,0,width,height);
  const bodyIn=ease(t/.5),gather=smooth((t-launchAt)/(wakeAt-launchAt)),eyeOpen=smooth((t-wakeAt)/.23),flash=Math.exp(-(((t-wakeAt)/.19)**2));
  let impact=0;
  for(const p of particles){const age=t-(launchAt+p.delay+p.duration);if(p.size>=4.5&&age>=0&&age<.24)impact+=Math.sin(age/.24*Math.PI)*Math.exp(-age*7)*(p.size/6.5)}
  impact=Math.min(1,impact);
  const blinkAge=t-(wakeAt+.27),blink=blinkAge>=0&&blinkAge<.15?1-.96*Math.sin(blinkAge/.15*Math.PI):1;
  const landingAge=t-(travelAt+.56),landing=target&&landingAge>=0&&landingAge<.18?Math.sin(landingAge/.18*Math.PI):0;
  body.setAttribute('transform',`translate(22 ${23+landing*.6}) scale(${1+impact*.025+landing*.08} ${1+impact*.018-landing*.06})`);
  shadow.setAttribute('transform',`translate(22 45) scale(${1+landing*.1} 1) translate(-22 -45)`);
  const bodyScale=3.2*(.88+.12*bodyIn)*(1-.09*flash)+.14*flash;
  let dx=0,dy=0,travel=0,scale=bodyScale;
  if(t>=travelAt){if(!target){target=document.querySelector('.robot-home .robot-face');if(target)targetOpacity=target.style.opacity}travel=smooth((t-travelAt)/.56);if(target?.isConnected){const r=target.getBoundingClientRect();dx=(r.left+r.width/2-cx)*travel;dy=(r.top+r.height/2-cy)*travel;scale=bodyScale*(1-travel)+(r.width/44)*travel}}
  hero.style.transform=`translate(-50%,-50%) translate(${dx}px,${dy}px) scale(${scale})`;
  const handoff=smooth((t-(endAt-.13))/.11);
  hero.style.opacity=String(1-handoff);if(target)target.style.opacity=String(handoff);
  eyes.style.opacity=String(eyeOpen);eyes.style.transform=`scaleY(${Math.max(.015,eyeOpen*blink)})`;
  halo.style.opacity=String(clamp(.32*smooth((t-.8)/.7)+impact*.35+flash*.9)*(1-travel));halo.style.transform=`translate(-50%,-50%) scale(${.65+gather*.35+impact*.08+flash*.22})`;
  const bgFade=smooth((t-(travelAt+.05))/.45);host.style.setProperty('--ss-bg-opacity',String(1-bgFade));host.style.opacity=String(1-smooth((t-(endAt-.13))/.11));
  label.style.opacity=String(.68*(1-smooth((t-(travelAt-.2))/.35)));
  if(t<wakeAt){for(let i=0;i<particles.length;i++){const p=particles[i],birth=smooth((t-.25-p.birth)/.4),u=clamp((t-launchAt-p.delay)/p.duration),approach=u**2.65,fade=1-smooth((approach-.95)/.05);if(birth*fade<.01)continue;
    const ox=p.x*width-cx,oy=p.y*height-cy,distance=Math.hypot(ox,oy)||1,nx=ox/distance,ny=oy/distance,bend=Math.sin(approach*Math.PI)*p.curve;
    const x=cx+ox*(1-approach)-ny*bend+Math.sin(t*1.8+i)*2*(1-approach),y=cy+oy*(1-approach)+nx*bend;
    const alpha=birth*fade*(.52+.48*rand(i+121)),speed=2.65*u**1.65*distance/p.duration,tail=Math.min(48,speed*.024);
    ctx.shadowColor=palette.glow;ctx.shadowBlur=5+p.size*2;
    if(tail>2){const trail=ctx.createLinearGradient(x,y,x+nx*tail,y+ny*tail);trail.addColorStop(0,`rgba(${palette.trail},${alpha*.7})`);trail.addColorStop(1,`rgba(${palette.trail},0)`);ctx.beginPath();ctx.strokeStyle=trail;ctx.lineWidth=Math.max(.7,p.size*.7);ctx.moveTo(x,y);ctx.lineTo(x+nx*tail,y+ny*tail);ctx.stroke()}
    ctx.beginPath();ctx.fillStyle=`rgba(${palette.core},${alpha})`;ctx.arc(x,y,p.size*(1-.25*approach),0,Math.PI*2);ctx.fill();
  }}
  if(flash>.01){ctx.beginPath();ctx.strokeStyle=`rgba(${palette.ring},${flash*.5})`;ctx.lineWidth=2;ctx.arc(cx,cy,39+75*(1-flash),0,Math.PI*2);ctx.stroke()}
  if(t>=endAt){finish();return}requestAnimationFrame(frame)
 }
 requestAnimationFrame(frame);
})();
