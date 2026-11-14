// Independent dynamic EMF remove/delete observations; verification only.
import java.nio.file.*;
import java.util.*;
import org.eclipse.emf.common.util.URI;
import org.eclipse.emf.ecore.*;
import org.eclipse.emf.ecore.resource.impl.*;
import org.eclipse.emf.ecore.util.EcoreUtil;
public class ObserveDeletion {
  static Map<String,Object> row(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int i=0;i<pairs.length;i+=2) result.put((String)pairs[i],pairs[i+1]);
    return result;
  }
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(ObserveDeletion::json).toList())+"]";
    if(value instanceof Boolean || value instanceof Number) return value.toString();
    return "\""+value.toString().replace("\\","\\\\").replace("\"","\\\"")+"\"";
  }
  static Object value(Object v,Map<EObject,String> ids) {
    if(v instanceof EObject o) return ids.get(o);
    if(v instanceof Collection<?> list) return list.stream().map(x->value(x,ids)).toList();
    return v;
  }
  @SuppressWarnings("unchecked")
  public static void main(String[] args) throws Exception {
    List<Object> cases=new ArrayList<>();
    for(boolean unsettable:List.of(false,true)) for(String operation:List.of("remove","delete-object","delete-subtree")) {
      var f=EcoreFactory.eINSTANCE;
      var p=f.createEPackage();p.setName("deletion");p.setNsPrefix("d");p.setNsURI("urn:deletion");
      var c=f.createEClass();c.setName("Node");p.getEClassifiers().add(c);
      var children=f.createEReference();children.setName("children");children.setEType(c);children.setUpperBound(-1);children.setContainment(true);children.setUnsettable(unsettable);
      var parent=f.createEReference();parent.setName("parent");parent.setEType(c);parent.setUnsettable(unsettable);
      var link=f.createEReference();link.setName("link");link.setEType(c);link.setUnsettable(unsettable);
      c.getEStructuralFeatures().addAll(List.of(children,parent,link));children.setEOpposite(parent);parent.setEOpposite(children);
      Map<String,EObject> objects=new LinkedHashMap<>();Map<EObject,String> ids=new IdentityHashMap<>();
      for(String name:List.of("a","b","c","d","e")) {var o=p.getEFactoryInstance().create(c);objects.put(name,o);ids.put(o,name);}
      var a=objects.get("a");var b=objects.get("b");var child=objects.get("c");var d=objects.get("d");var e=objects.get("e");
      var set=new ResourceSetImpl();var first=new ResourceImpl(URI.createURI("one.xmi"));var second=new ResourceImpl(URI.createURI("two.xmi"));set.getResources().addAll(List.of(first,second));first.getContents().addAll(List.of(a,d));second.getContents().add(e);
      ((List<EObject>)a.eGet(children)).add(b);((List<EObject>)b.eGet(children)).add(child);d.eSet(link,b);e.eSet(link,child);
      if(operation.equals("remove")) EcoreUtil.remove(b);else EcoreUtil.delete(b,operation.equals("delete-subtree"));
      Map<String,Object> states=new LinkedHashMap<>();
      objects.forEach((name,o)->states.put(name,row("children",value(o.eGet(children),ids),"parent",value(o.eGet(parent),ids),"link",value(o.eGet(link),ids),"children_set",o.eIsSet(children),"parent_set",o.eIsSet(parent),"link_set",o.eIsSet(link),"resource",o.eResource()==null?null:o.eResource().getURI().toString())));
      cases.add(row("unsettable",unsettable,"operation",operation,"objects",states,"roots_one",value(first.getContents(),ids),"roots_two",value(second.getContents(),ids)));
    }
    Files.writeString(Path.of(args[0]),json(row("schema_version",1,"cases",cases))+"\n");
  }
}
