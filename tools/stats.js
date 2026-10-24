const fs=require('fs');
const R=(p)=>JSON.parse(fs.readFileSync(p,'utf8'));
const rubros=R('data/rubros.json'), listado=R('data/empresas_listado.json'), det=R('data/empresas_detalle.json');
const rb=R('data.bak/rubros.json'), lb=R('data.bak/empresas_listado.json');
const slugs=new Set(listado.map(e=>e.slug));
const prev=new Set(lb.map(e=>e.slug));
const nuevos=[...slugs].filter(s=>!prev.has(s));
const nuevosRubros=rubros.filter(r=>!rb.some(x=>x.slug===r.slug));
console.log('== RUBROS ==', rubros.length, '| nuevos:', nuevosRubros.map(r=>r.nombre).join(', '));
console.log('== EMPRESAS ==', listado.length, '| nuevas:', nuevos.length);
const c=(f)=>listado.filter(f).length;
console.log('con direccion:', c(e=>e.direccion));
console.log('con telefonos:', c(e=>e.telefonos&&e.telefonos.length));
console.log('con desc corta:', c(e=>e.descripcion_corta));
console.log('con imagen perfil:', c(e=>e.imagen_perfil), '| portada:', c(e=>e.imagen_portada));
const rels=listado.reduce((a,e)=>a+(e.rubros?e.rubros.length:0),0);
console.log('relaciones:', rels, '| promedio rubros/empresa:', (rels/listado.length).toFixed(1));
const niv={};for(const e of listado)niv[e.nivel]=(niv[e.nivel]||0)+1;console.log('niveles:',JSON.stringify(niv));
let tels=0,wa=0;for(const e of listado)for(const t of (e.telefonos||[])){tels++;if(t.whatsapp)wa++;}
console.log('telefonos totales:', tels, '| whatsapp:', wa);
let fotos=0,desc=0,cp=0,err=0,multifoto=0;
for(const k in det){const d=det[k];if(d.error){err++;continue;}if(d.fotos&&d.fotos.length){fotos+=d.fotos.length;if(d.fotos.length>1)multifoto++;}if(d.descripcion)desc++;if(d.codigo_postal)cp++;}
console.log('detalles: con descripcion larga:', desc, '| con CP:', cp, '| fotos totales:', fotos, '| empresas con fotos:', multifoto, '| errores:', err);
console.log('slugs en detalle:', Object.keys(det).length);
console.log('--- ejemplo ---');
const ej=listado.find(e=>e.nivel==='premium')||listado[0];
console.log(JSON.stringify(ej,null,1).slice(0,900));
