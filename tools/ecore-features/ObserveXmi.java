// Handwritten independent EMF save/load oracle. Verification only; no runtime dependency.
import java.nio.file.*;
import java.io.*;
import java.util.*;
import org.eclipse.emf.common.util.URI;
import org.eclipse.emf.ecore.*;
import org.eclipse.emf.ecore.resource.impl.ResourceSetImpl;
import org.eclipse.emf.ecore.xmi.impl.*;

public class ObserveXmi {
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(ObserveXmi::json).toList())+"]";
    if(value instanceof Boolean || value instanceof Number) return value.toString();
    return "\""+value.toString().replace("\\","\\\\").replace("\"","\\\"").replace("\n","\\n").replace("\r","\\r").replace("\t","\\t")+"\"";
  }
  static Map<String,Object> record(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int i=0;i<pairs.length;i+=2) result.put((String)pairs[i],pairs[i+1]);
    return result;
  }
  static Object value(Object input, XMIResourceImpl resource) {
    if(input instanceof EObject object) return resource.getID(object);
    if(input instanceof Collection<?> values) return values.stream().map(v->value(v,resource)).toList();
    return input;
  }
  static Map<String,Object> observe(EObject object,XMIResourceImpl resource) {
    Map<String,Object> fields=new LinkedHashMap<>();
    for(EStructuralFeature field:object.eClass().getEAllStructuralFeatures())
      fields.put(field.getName(),record("value",value(object.eGet(field),resource),"is_set",object.eIsSet(field)));
    return fields;
  }
  public static void main(String[] args) throws Exception {
    Path output=Path.of(args[0]);Files.createDirectories(output);
    EcoreFactory factory=EcoreFactory.eINSTANCE;
    EPackage pack=factory.createEPackage();pack.setName("state");pack.setNsPrefix("state");pack.setNsURI("urn:intersect:e09:xmi");
    EClass type=factory.createEClass();type.setName("Item");pack.getEClassifiers().add(type);
    for(String name:List.of("value","nullable","tags","link","links","child","children")) {
      boolean reference=List.of("link","links","child","children").contains(name);
      EStructuralFeature feature=reference?factory.createEReference():factory.createEAttribute();
      feature.setName(name);feature.setEType(reference?type:EcorePackage.Literals.ESTRING);feature.setUnsettable(true);
      if(List.of("tags","links","children").contains(name)) feature.setUpperBound(-1);
      if(name.equals("value")) feature.setDefaultValueLiteral("seed");
      if(reference) ((EReference)feature).setContainment(name.startsWith("child"));
      type.getEStructuralFeatures().add(feature);
    }
    org.eclipse.emf.ecore.resource.Resource schema=new EcoreResourceFactoryImpl().createResource(URI.createURI("schema.ecore"));schema.getContents().add(pack);
    try(OutputStream out=Files.newOutputStream(output.resolve("schema.ecore"))) {schema.save(out,Map.of());}
    String[][] inputs={{"fresh","",""},{"scalar-default","value","seed"},{"scalar-null","value","NULL"},{"scalar-value","value","custom"},
      {"nullable-null","nullable","NULL"},{"nullable-empty","nullable",""},{"tags-empty","tags","EMPTY"},{"tags-one","tags","x"},{"tags-empty-item","tags",""},
      {"link-null","link","NULL"},{"link-target","link","TARGET"},{"links-empty","links","EMPTY"},{"links-target","links","TARGET"},
      {"child-null","child","NULL"},{"child-target","child","TARGET"},{"children-empty","children","EMPTY"},{"children-target","children","TARGET"}};
    List<Object> cases=new ArrayList<>();
    for(String[] input:inputs) {
      String file=input[0]+".xmi";
      XMIResourceImpl resource=new XMIResourceImpl(URI.createURI(file));
      EObject object=pack.getEFactoryInstance().create(type);resource.getContents().add(object);resource.setID(object,"subject");
      if(!input[1].isEmpty()) {
        EStructuralFeature field=type.getEStructuralFeature(input[1]);
        Object assigned=input[2].equals("NULL")?null:input[2];
        if(input[2].equals("TARGET")) {
          EObject target=pack.getEFactoryInstance().create(type);
          if(!((EReference)field).isContainment()) resource.getContents().add(target);
          assigned=target;
        }
        object.eSet(field,field.isMany()?(input[2].equals("EMPTY")?List.of():List.of(assigned)):assigned);
        if(assigned instanceof EObject target) resource.setID(target,"target");
      }
      Map<String,Object> before=observe(object,resource);
      ByteArrayOutputStream bytes=new ByteArrayOutputStream();resource.save(bytes,Map.of());
      Files.write(output.resolve(file),bytes.toByteArray());
      ResourceSetImpl set=new ResourceSetImpl();set.getPackageRegistry().put(pack.getNsURI(),pack);
      XMIResourceImpl loaded=new XMIResourceImpl(URI.createURI(file));set.getResources().add(loaded);
      loaded.load(new ByteArrayInputStream(bytes.toByteArray()),Map.of());
      Map<String,Object> after=observe(loaded.getEObject("subject"),loaded);
      if(!before.equals(after)) throw new IllegalStateException("Lost state in "+file+": "+json(before)+" -> "+json(after));
      if(args.length>1) {
        Path nativeFile=Path.of(args[1]).resolve(input[0]).resolve("resource-0000.xmi");
        ResourceSetImpl nativeSet=new ResourceSetImpl();nativeSet.getPackageRegistry().put(pack.getNsURI(),pack);
        XMIResourceImpl nativeLoaded=new XMIResourceImpl(URI.createFileURI(nativeFile.toAbsolutePath().toString()));nativeSet.getResources().add(nativeLoaded);
        try(InputStream in=Files.newInputStream(nativeFile)) { nativeLoaded.load(in,Map.of()); }
        org.eclipse.emf.ecore.util.EcoreUtil.resolveAll(nativeLoaded);
        if(!nativeLoaded.getErrors().isEmpty()) throw new IllegalStateException(nativeLoaded.getErrors().toString());
        EObject subject=nativeLoaded.getContents().get(0);
        for(EObject root:nativeLoaded.getContents()) {
          nativeLoaded.setID(root,root==subject?"subject":"target");
          for(var iterator=root.eAllContents();iterator.hasNext();) nativeLoaded.setID(iterator.next(),"target");
        }
        Map<String,Object> nativeState=observe(subject,nativeLoaded);
        if(!before.equals(nativeState)) throw new IllegalStateException("Native export lost state in "+file+": "+json(before)+" -> "+json(nativeState));
      }
      cases.add(record("name",input[0],"file",file,"before",before,"after",after));
    }
    if(args.length>1) Files.writeString(output.resolve("native-verification.json"),json(record("verified",cases.size(),"cases",cases.stream().map(c->((Map<?,?>)c).get("name")).toList()))+"\n");
    Files.writeString(output.resolve("observations.json"),json(record("schema_version",1,"cases",cases))+"\n");
  }
}
