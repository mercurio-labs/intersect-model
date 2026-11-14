// Independent Ecore declaration validity and same-object inverse observations.
// Verification only; never linked into normal builds or production.
import java.nio.file.*;
import java.util.*;
import org.eclipse.emf.common.util.Diagnostic;
import org.eclipse.emf.ecore.*;
import org.eclipse.emf.ecore.util.Diagnostician;
public class ObserveReferenceIdentity {
  static Map<String,Object> row(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int i=0;i<pairs.length;i+=2) result.put((String)pairs[i],pairs[i+1]);
    return result;
  }
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(ObserveReferenceIdentity::json).toList())+"]";
    if(value instanceof Boolean || value instanceof Number) return value.toString();
    return "\""+value.toString().replace("\\","\\\\").replace("\"","\\\"")+"\"";
  }
  static Object value(Object v,Map<EObject,String> ids) {
    if(v instanceof EObject o) return ids.get(o);
    if(v instanceof Collection<?> list) return list.stream().map(x->value(x,ids)).toList();
    return v;
  }
  static Map<String,Object> state(Map<String,EObject> objects,Map<EObject,String> ids,EReference out,EReference back) {
    Map<String,Object> result=new LinkedHashMap<>();
    objects.forEach((name,o)->result.put(name,row("out",value(o.eGet(out),ids),"back",value(o.eGet(back),ids),"out_set",o.eIsSet(out),"back_set",o.eIsSet(back))));
    return result;
  }
  static void assign(EObject o,EReference f,EObject target) { o.eSet(f,f.isMany()?List.of(target):target); }
  public static void main(String[] args) throws Exception {
    List<Object> invalid=new ArrayList<>(),cases=new ArrayList<>();
    for(boolean many:List.of(false,true)) for(boolean unsettable:List.of(false,true)) {
      var f=EcoreFactory.eINSTANCE;
      var p=f.createEPackage();p.setName("identity");p.setNsPrefix("i");p.setNsURI("urn:identity");
      var c=f.createEClass();c.setName("Node");p.getEClassifiers().add(c);
      var out=f.createEReference();out.setName("out");out.setEType(c);out.setUpperBound(many?-1:1);out.setUnsettable(unsettable);c.getEStructuralFeatures().add(out);
      out.setEOpposite(out);
      var invalidDiagnostic=Diagnostician.INSTANCE.validate(p);
      invalid.add(row("many",many,"unsettable",unsettable,"valid",invalidDiagnostic.getSeverity()<Diagnostic.ERROR,"codes",invalidDiagnostic.getChildren().stream().map(Diagnostic::getCode).toList()));
      out.setEOpposite(null);
      var back=f.createEReference();back.setName("back");back.setEType(c);back.setUnsettable(unsettable);c.getEStructuralFeatures().add(back);out.setEOpposite(back);back.setEOpposite(out);
      var diagnostic=Diagnostician.INSTANCE.validate(p);
      if(diagnostic.getSeverity()>=Diagnostic.ERROR) throw new IllegalStateException(diagnostic.toString());
      Map<String,EObject> objects=new LinkedHashMap<>();Map<EObject,String> ids=new IdentityHashMap<>();
      for(String name:List.of("a","b")) { var o=p.getEFactoryInstance().create(c);objects.put(name,o);ids.put(o,name); }
      var a=objects.get("a");var b=objects.get("b");List<Object> steps=new ArrayList<>();
      steps.add(row("operation","fresh","objects",state(objects,ids,out,back)));
      assign(a,out,b);steps.add(row("operation","bind-other","objects",state(objects,ids,out,back)));
      assign(a,out,a);steps.add(row("operation","bind-self","objects",state(objects,ids,out,back)));
      assign(b,out,a);steps.add(row("operation","displace-self","objects",state(objects,ids,out,back)));
      cases.add(row("many",many,"unsettable",unsettable,"steps",steps));
    }
    Files.writeString(Path.of(args[0]),json(row("schema_version",1,"invalid_declarations",invalid,"cases",cases))+"\n");
  }
}
