// Independent EMF inverse-state XMI observation host. Verification only.
import java.nio.file.*;
import java.io.*;
import java.util.*;
import org.eclipse.emf.common.util.URI;
import org.eclipse.emf.ecore.*;
import org.eclipse.emf.ecore.resource.impl.ResourceSetImpl;
import org.eclipse.emf.ecore.xmi.impl.*;
public class ObserveInverseXmi {
  static Map<String,Object> record(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int n=0;n<pairs.length;n+=2) result.put((String)pairs[n],pairs[n+1]);
    return result;
  }
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(ObserveInverseXmi::json).toList())+"]";
    if(value instanceof Boolean || value instanceof Number) return value.toString();
    return "\""+value.toString().replace("\\","\\\\").replace("\"","\\\"").replace("\n","\\n").replace("\r","\\r").replace("\t","\\t")+"\"";
  }

  static Object ids(Object value, IdentityHashMap<EObject,String> ids) {
    if(value instanceof EObject object) return ids.get(object);
    if(value instanceof Collection<?> values) return values.stream().map(v->ids(v,ids)).toList();
    return value;
  }

  static Map<String,Object> observe(Map<String,EObject> objects) {
    IdentityHashMap<EObject,String> names=new IdentityHashMap<>();objects.forEach((n,o)->names.put(o,n));
    Map<String,Object> states=new LinkedHashMap<>();
    objects.forEach((name,object)-> {
      EStructuralFeature field=object.eClass().getEStructuralFeatures().get(0);
      states.put(name,record("class",object.eClass().getName(),"value",ids(object.eGet(field),names),"is_set",object.eIsSet(field),"container",ids(object.eContainer(),names)));
    });return states;
  }
  static void assign(EObject owner,EReference feature,EObject value) {
    owner.eSet(feature,feature.isMany()?(value==null?List.of():List.of(value)):value);
  }
  public static void main(String[] args) throws Exception {
    Path output=Path.of(args[0]);Files.createDirectories(output);List<Object> cases=new ArrayList<>();List<Object> nativeCases=new ArrayList<>();
    for(boolean containment:List.of(false,true)) for(boolean outMany:List.of(false,true)) for(boolean backMany:List.of(false,true)) {
      if(containment && backMany) continue;
      for(int flags=0;flags<4;flags++) {
        String name="c"+(containment?1:0)+"-o"+(outMany?1:0)+"-b"+(backMany?1:0)+"-u"+flags;
        EcoreFactory f=EcoreFactory.eINSTANCE;EPackage pack=f.createEPackage();pack.setName("inverse");pack.setNsURI("urn:inverse:xmi:"+name);pack.setNsPrefix("inverse");
        EClass a=f.createEClass();a.setName("A");EClass b=f.createEClass();b.setName("B");pack.getEClassifiers().addAll(List.of(a,b));
        EReference out=f.createEReference();out.setName("out");out.setEType(b);out.setUpperBound(outMany?-1:1);out.setContainment(containment);out.setUnsettable((flags&1)!=0);a.getEStructuralFeatures().add(out);
        EReference back=f.createEReference();back.setName("back");back.setEType(a);back.setUpperBound(backMany?-1:1);back.setUnsettable((flags&2)!=0);b.getEStructuralFeatures().add(back);out.setEOpposite(back);back.setEOpposite(out);
        var schema=new EcoreResourceFactoryImpl().createResource(URI.createURI("memory:/"+name+".ecore"));schema.getContents().add(pack);
        try(OutputStream stream=Files.newOutputStream(output.resolve(name+".ecore"))) {schema.save(stream,Map.of());}
        XMIResourceImpl resource=new XMIResourceImpl(URI.createURI("memory:/"+name+".xmi"));Map<String,EObject> objects=new LinkedHashMap<>();
        for(String scenario:List.of("fresh","linked","remove-forward","remove-inverse","empty-back","reset-forward")) {
          EObject left=pack.getEFactoryInstance().create(a),right=pack.getEFactoryInstance().create(b);
          resource.getContents().add(left);
          switch(scenario) {
            case "linked" -> assign(left,out,right);
            case "remove-forward" -> {assign(left,out,right);assign(left,out,null);}
            case "remove-inverse" -> {assign(left,out,right);right.eUnset(back);}
            case "empty-back" -> assign(right,back,null);
            case "reset-forward" -> {assign(left,out,right);left.eUnset(out);}
          }
          if(right.eContainer()==null && right.eResource()!=resource) resource.getContents().add(right);
          if(scenario.equals("empty-back")) assign(right,back,null);
          if(right.eContainer()!=null && resource.getContents().contains(right)) throw new IllegalStateException("Owned child must not remain a resource root");
          resource.setID(left,scenario+"-a");resource.setID(right,scenario+"-b");objects.put(scenario+"-a",left);objects.put(scenario+"-b",right);
        }
        Map<String,Object> before=observe(objects);ByteArrayOutputStream bytes=new ByteArrayOutputStream();resource.save(bytes,Map.of());Files.write(output.resolve(name+".xmi"),bytes.toByteArray());
        Map<String,Object> after=null;String loadError=null;
        try {
          ResourceSetImpl set=new ResourceSetImpl();set.getPackageRegistry().put(pack.getNsURI(),pack);XMIResourceImpl loaded=new XMIResourceImpl(resource.getURI());set.getResources().add(loaded);loaded.load(new ByteArrayInputStream(bytes.toByteArray()),Map.of());org.eclipse.emf.ecore.util.EcoreUtil.resolveAll(loaded);
          Map<String,EObject> afterObjects=new LinkedHashMap<>();for(String id:objects.keySet()) afterObjects.put(id,loaded.getEObject(id));
          after=observe(afterObjects);
        } catch(Exception error) {loadError=error.getClass().getName()+": "+error.getMessage();}
        if(args.length>1) {
          Path directory=Path.of(args[1]).resolve(name),nativeFile=directory.resolve("resource-0000.xmi");
          ResourceSetImpl nativeSet=new ResourceSetImpl();nativeSet.getPackageRegistry().put(pack.getNsURI(),pack);
          XMIResourceImpl loaded=new XMIResourceImpl(URI.createFileURI(nativeFile.toAbsolutePath().toString()));nativeSet.getResources().add(loaded);
          try(InputStream stream=Files.newInputStream(nativeFile)) {loaded.load(stream,Map.of());}
          org.eclipse.emf.ecore.util.EcoreUtil.resolveAll(loaded);
          if(!loaded.getErrors().isEmpty()) throw new IllegalStateException(loaded.getErrors().toString());
          Map<String,EObject> normalized=new LinkedHashMap<>();Set<EObject> unique=Collections.newSetFromMap(new IdentityHashMap<>());
          for(String line:Files.readAllLines(directory.resolve("ids.tsv"))) {
            String[] pair=line.split("\\t",-1);if(pair.length!=2) throw new IllegalStateException("Invalid identity mapping");
            EObject object=loaded.getEObject(pair[0]);
            if(object==null || !unique.add(object) || normalized.put(pair[1],object)!=null) throw new IllegalStateException("Missing or duplicate identity");
          }
          Set<EObject> all=Collections.newSetFromMap(new IdentityHashMap<>());
          for(var iterator=loaded.getAllContents();iterator.hasNext();) all.add(iterator.next());
          if(!all.equals(unique) || !normalized.keySet().equals(objects.keySet())) throw new IllegalStateException("Incomplete identity mapping");
          Map<String,Object> nativeState=observe(normalized);
          if(!before.equals(nativeState)) throw new IllegalStateException("Native export lost state in "+name+": "+json(before)+" -> "+json(nativeState));
          nativeCases.add(record("name",name,"objects",nativeState,"preserved",true));
        }
        cases.add(record("name",name,"namespace",pack.getNsURI(),"containment",containment,"out_many",outMany,"back_many",backMany,"out_unsettable",out.isUnsettable(),"back_unsettable",back.isUnsettable(),"order",new ArrayList<>(objects.keySet()),"before",before,"after",after,"load_error",loadError,"emf_preserved",after==null?null:before.equals(after)));

      }
    }
    if(args.length>1) Files.writeString(output.resolve("native-verification.json"),json(record("verified",nativeCases.size(),"cases",nativeCases))+"\n");
    Files.writeString(output.resolve("observations.json"),json(record("schema_version",1,"cases",cases))+"\n");
  }
}
