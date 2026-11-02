/* Spike harness, not a production player. Exact platform and fixture provenance belong in evidence. */
const invoke = window.__TAURI__.core.invoke;
const audio = document.querySelector('audio');
const output = document.querySelector('#output');
async function report(event) {
  // Bound the UI transcript so harness DOM growth does not dominate the memory experiment.
  output.textContent = (output.textContent + JSON.stringify(event) + '\n').slice(-16384);
  await invoke('report', {event});
}
const sleep = ms => new Promise(r=>setTimeout(r, ms));
function eventOnce(target, name, ms=10000) {
  return new Promise((resolve,reject)=> {
    const done = e => {clearTimeout(timer);target.removeEventListener(name,done);target.removeEventListener('error',fail);resolve(e);};
    const fail = () => {clearTimeout(timer);target.removeEventListener(name,done);target.removeEventListener('error',fail);reject(Error(`media error ${target.error?.code}`));};
    const timer=setTimeout(()=>{target.removeEventListener(name,done);target.removeEventListener('error',fail);reject(Error(`${name} timeout`));},ms);
    target.addEventListener(name,done,{once:true});target.addEventListener('error',fail,{once:true});
  });
}
let mediaBase;
function scheme(kind, query) {
  if(mediaBase)return mediaBase+kind+'?'+new URLSearchParams(query);
  // Custom protocol origin mapping differs on Windows/WebView2.
  const win = navigator.userAgent.includes('Windows');
  return (win ? `http://biu-media.localhost/${kind}` : `biu-media://${kind}`) + '?' + new URLSearchParams(query);
}
async function load(url) {
  audio.pause(); audio.src=url;
  const loaded=eventOnce(audio,'loadedmetadata'); audio.load(); await loaded;
  const playing=eventOnce(audio,'playing');await audio.play();await playing;
  await sleep(600);
}
let graph;
async function waveform() {
  if(!graph){
    const ctx=new AudioContext(),source=ctx.createMediaElementSource(audio),analyser=ctx.createAnalyser();source.connect(analyser);analyser.connect(ctx.destination);graph={ctx,analyser};
  }
  const {ctx,analyser}=graph;await ctx.resume();
  await sleep(1000);const samples=new Float32Array(analyser.fftSize);analyser.getFloatTimeDomainData(samples);
  await report({event:'waveform',max:Math.max(...samples.map(Math.abs)),state:ctx.state});
  // Leave the graph connected for the soak.
}
async function missing() {
  const start=performance.now();let updates=0;
  audio.addEventListener('timeupdate',()=>updates++);
  audio.src=scheme('local',{name:'aac.m4a'});audio.load();
  let error;
  audio.addEventListener('error',()=>{error=audio.error?.code;});
  audio.play().catch(()=>{});
  while (!error && updates===0 && performance.now()-start<2900) await sleep(10);
  const ms=performance.now()-start;
  if(error || updates===0) document.querySelector('#install').textContent='Unable to decode AAC. Ubuntu: sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-good';
  await report({event:'missing_decoder',ms,error,updates,detected:!!document.querySelector('#install').textContent});
  await invoke('finish');
}
async function run(config) {
  if(config.mode!=='custom')mediaBase=await invoke('media_base');
  await report({event:'transport',kind:mediaBase?'loopback-http-token':'custom-scheme'});
  await report({event:'environment',userAgent:navigator.userAgent,mediaSession:!!navigator.mediaSession,codecs:Object.fromEntries(['audio/mp4; codecs="mp4a.40.2"','audio/mp4; codecs="flac"','audio/mp4; codecs="fLaC"','audio/mp4; codecs="ec-3"','audio/flac'].map(m=>[m,audio.canPlayType(m)]))});
  if(config.mode==='missing')return missing();
  if(config.mode==='verify-update') {
    await report(await invoke('update_verify'));await invoke('finish');return;
  }
  if(config.mode==='update') {
    await invoke('update_test');return;
  }
  if(config.mode==='controls') {
    await window.__TAURI__.event.listen('native-media', async ({payload})=>{
      if(payload==='Pause')audio.pause();
      if(payload==='Play')await audio.play();
      if(payload==='Toggle'){if(audio.paused)await audio.play();else audio.pause();}
      await report({event:'native_frontend_action',action:payload,paused:audio.paused});
    });
    const image=new Image();image.src='https://i0.hdslb.com/bfs/face/member/noface.jpg';image.onload=()=>report({event:'hdslb_image',loaded:true,width:image.naturalWidth});image.onerror=()=>report({event:'hdslb_image',loaded:false});document.body.append(image);
    await load(scheme('local',{name:'aac.m4a'}));await report({event:'controls_playing'});
    await sleep(20000);await invoke('finish');return;
  }
  await report({event:'dependency_probe',result:await invoke('dependency_probe')});
  await invoke('lyrics');
  // Test against Range-aware loopback upstream; CDN/account tests remain separate.
  await load(scheme('remote',{u:'http://127.0.0.1:18765/expired'}));
  await report({event:'aac_playing',duration:audio.duration,time:audio.currentTime});
  if(navigator.mediaSession) {
    navigator.mediaSession.metadata=new MediaMetadata({title:'Biu M0 sine fixture',artist:'Synthetic test',album:'M0',artwork:[{src:'http://127.0.0.1:18765/art.svg',sizes:'128x128',type:'image/svg+xml'}]});
    navigator.mediaSession.playbackState='playing';
    for(const action of ['play','pause','previoustrack','nexttrack'])navigator.mediaSession.setActionHandler(action,()=>report({event:'media_action',action}));
  }
  let seed=2401;
  const targets=Array.from({length:10},()=>{seed=(Math.imul(1664525,seed)+1013904223)>>>0;return 10+Math.floor(seed/4294967296*280);});
  await report({event:'seek_seed',seed:2401,targets});
  for(const target of targets) {
    const start=performance.now();const done=eventOnce(audio,'seeked');audio.currentTime=target;await done;
    const position=audio.currentTime;
    if(Math.abs(position-target)>1)throw Error(`seek target mismatch: ${position} vs ${target}`);
    while(audio.currentTime<=position+0.03) { if(performance.now()-start>8000)throw Error('seek playback stalled'); await sleep(10); }
    await report({event:'seek',target,ms:performance.now()-start,time:audio.currentTime});await sleep(200);
  }
  await waveform();
  await load(scheme('local',{name:'tone.flac'}));await sleep(2000);
  await report({event:'flac_playing',duration:audio.duration,time:audio.currentTime});
  audio.loop=false;
  await report({event:'soak_start',seconds:config.seconds,kind:'reload-and-range',round_seconds:30,wall_time_ms:Date.now()});
  const until=performance.now()+config.seconds*1000;
  let round=0;
  while(performance.now()<until) {
    round++;
    const roundStart=performance.now();
    audio.pause();audio.removeAttribute('src');audio.load();
    await load(scheme('remote',{u:`http://127.0.0.1:18765/aac.m4a?round=${round}`,round}));
    const target=170+(round*31)%100,start=performance.now(),done=eventOnce(audio,'seeked');audio.currentTime=target;await done;
    const position=audio.currentTime;
    if(Math.abs(position-target)>2)throw Error('soak seek target mismatch');
    while(audio.currentTime<=position+0.03){if(performance.now()-start>8000)throw Error('soak seek stalled');await sleep(10);}
    await report({event:'soak_reload',round,target,ms:performance.now()-start,time:audio.currentTime,wall_time_ms:Date.now()});
    const roundUntil=Math.min(until,roundStart+30000);
    while(performance.now()<roundUntil){await sleep(Math.min(10000,roundUntil-performance.now()));await report({event:'soak',round,time:audio.currentTime,paused:audio.paused,error:audio.error?.code||null,wall_time_ms:Date.now()});}
  }
  audio.pause();audio.removeAttribute('src');audio.load();await sleep(500);
  await report({event:'complete',rounds:round});await invoke('finish');
}
document.querySelector('#sample-run').onclick=async()=>{
  const name=document.querySelector('#sample').value;
  try {
    mediaBase=undefined;
    await report({event:'sample_start',name,codecs:Object.fromEntries(['audio/mp4; codecs="flac"','audio/mp4; codecs="fLaC"','audio/mp4; codecs="ec-3"'].map(m=>[m,audio.canPlayType(m)]))});
    await load(scheme('local',{name}));const start=performance.now(),startTime=audio.currentTime;
    await sleep(60000);
    if(audio.paused||audio.error||audio.currentTime<startTime+58)throw Error('60 s continuous playback failed');
    await report({event:'sample_continuous',name,ms:performance.now()-start,time:audio.currentTime});
    for(const fraction of [0.7,0.1,0.5,0.3,0.8]){const target=audio.duration*fraction,start=performance.now();const done=eventOnce(audio,'seeked');audio.currentTime=target;await done;await sleep(150);if(audio.paused||audio.error||Math.abs(audio.currentTime-target)>2)throw Error('sample seek failed');await report({event:'sample_seek',name,target,ms:performance.now()-start});}
    await waveform();await report({event:'sample_complete',name});
  }catch(e){await report({event:'sample_failed',name,message:String(e)});}
};
document.querySelector('#lyrics').onclick=()=>invoke('lyrics');
document.querySelector('#run').onclick=()=>run({seconds:60}).catch(e=>report({event:'failure',message:String(e)}));
invoke('mode').then(config=>{if(config.mode!=='interactive')run(config).catch(async e=>{await report({event:'failure',message:String(e)});await invoke('finish');});});
