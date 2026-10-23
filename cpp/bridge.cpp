// Ponte C++ verso OpenCascade (OCCT): lettura STEP/IGES con colori e nomi (XDE), assiemi, triangolazione.
// OpenCascade e' distribuito con licenza LGPL 2.1 con l'eccezione OCCT.

#include "bridge.h"

#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepTools.hxx>
#include <BRep_Tool.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <IGESCAFControl_Reader.hxx>
#include <IGESCAFControl_Writer.hxx>
#include <Interface_Static.hxx>
#include <OSD_Parallel.hxx>
#include <OSD_ThreadPool.hxx>
#include <Poly_Triangulation.hxx>
#include <Quantity_Color.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <STEPCAFControl_Writer.hxx>
#include <STEPControl_StepModelType.hxx>
#include <Standard_Failure.hxx>
#include <Standard_Version.hxx>
#include <TCollection_AsciiString.hxx>
#include <TCollection_ExtendedString.hxx>
#include <TDF_Label.hxx>
#include <TDF_LabelSequence.hxx>
#include <TDF_Tool.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <TopExp_Explorer.hxx>
#include <TopLoc_Location.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_ColorType.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <map>
#include <string>
#include <vector>

namespace {

void set_err(char* err, uint32_t len, const std::string& m) {
  if (!err || len == 0) return;
  std::snprintf(err, len, "%s", m.c_str());
}

// UTF-16 (nomi di OpenCascade) -> UTF-8
std::string to_utf8(const TCollection_ExtendedString& s) {
  std::string out;
  const Standard_ExtString p = s.ToExtString();
  const int n = s.Length();
  for (int i = 0; i < n; ++i) {
    uint32_t c = p[i];
    if (c >= 0xD800 && c <= 0xDBFF && i + 1 < n) {
      uint32_t d = p[i + 1];
      if (d >= 0xDC00 && d <= 0xDFFF) {
        c = 0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00);
        ++i;
      }
    }
    if (c < 0x80) {
      out += static_cast<char>(c);
    } else if (c < 0x800) {
      out += static_cast<char>(0xC0 | (c >> 6));
      out += static_cast<char>(0x80 | (c & 0x3F));
    } else if (c < 0x10000) {
      out += static_cast<char>(0xE0 | (c >> 12));
      out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
      out += static_cast<char>(0x80 | (c & 0x3F));
    } else {
      out += static_cast<char>(0xF0 | (c >> 18));
      out += static_cast<char>(0x80 | ((c >> 12) & 0x3F));
      out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
      out += static_cast<char>(0x80 | (c & 0x3F));
    }
  }
  return out;
}

std::string label_name(const TDF_Label& l) {
  Handle(TDataStd_Name) n;
  if (l.FindAttribute(TDataStd_Name::GetID(), n)) return to_utf8(n->Get());
  return std::string();
}

struct Bucket {
  std::array<float, 4> rgba{{-1.f, -1.f, -1.f, -1.f}};
  std::vector<float> v;
  std::vector<uint32_t> idx;
};

struct Ctx {
  Handle(XCAFDoc_ShapeTool) st;
  Handle(XCAFDoc_ColorTool) ct;
  double lin = 0.1, ang = 0.5;
  rstp_item_cb cb = nullptr;
  void* user = nullptr;
  bool instances = false;
  std::map<std::string, int> seen;  // quante volte e' gia' comparso un nome (istanze dello stesso pezzo)
  std::map<std::string, int> uses;  // quante volte compare ogni pezzo (forma semplice) nell'assieme
  std::vector<std::string> unique_entries;  // i pezzi, ciascuno una volta: si triangolano una volta sola
  std::vector<TopoDS_Shape> unique_shapes;
  std::map<std::string, std::vector<uint32_t>> defs;  // pezzo ripetuto -> le sue definizioni (una per colore)
  uint32_t next_id = 1;
};

std::string entry_of(const TDF_Label& l) {
  TCollection_AsciiString e;
  TDF_Tool::Entry(l, e);
  return std::string(e.ToCString());
}

bool color_of(const Ctx& c, const TDF_Label& l, Quantity_Color& out) {
  return c.ct->GetColor(l, XCAFDoc_ColorSurf, out) || c.ct->GetColor(l, XCAFDoc_ColorGen, out);
}

bool color_of(const Ctx& c, const TopoDS_Shape& s, Quantity_Color& out) {
  return c.ct->GetColor(s, XCAFDoc_ColorSurf, out) || c.ct->GetColor(s, XCAFDoc_ColorGen, out);
}

std::array<float, 4> to_rgba(const Quantity_Color& q) {
  double r, g, b;
  q.Values(r, g, b, Quantity_TOC_sRGB);
  return {{static_cast<float>(r), static_cast<float>(g), static_cast<float>(b), 1.f}};
}

// I triangoli di un pezzo, divisi per colore, portati con la posizione `parent`.
std::map<std::array<int, 4>, Bucket> collect(Ctx& c, const TDF_Label& label, const TopLoc_Location& parent) {
  std::map<std::array<int, 4>, Bucket> buckets;
  TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(label);
  if (shape.IsNull()) return buckets;
  BRepMesh_IncrementalMesh mesher(shape, c.lin, Standard_False, c.ang, Standard_True);  // gia' fatto: non rifa' nulla

  Quantity_Color part_color;
  const bool has_part_color = color_of(c, label, part_color);

  for (TopExp_Explorer ex(shape, TopAbs_FACE); ex.More(); ex.Next()) {
    const TopoDS_Face face = TopoDS::Face(ex.Current());
    TopLoc_Location face_loc;
    Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(face, face_loc);
    if (tri.IsNull()) continue;

    std::array<float, 4> rgba{{-1.f, -1.f, -1.f, -1.f}};
    Quantity_Color fc;
    if (color_of(c, face, fc)) rgba = to_rgba(fc);
    else if (has_part_color) rgba = to_rgba(part_color);
    const std::array<int, 4> key{{static_cast<int>(rgba[0] * 255.f + 0.5f), static_cast<int>(rgba[1] * 255.f + 0.5f),
                                 static_cast<int>(rgba[2] * 255.f + 0.5f), static_cast<int>(rgba[3] * 255.f + 0.5f)}};
    Bucket& b = buckets[key];
    b.rgba = rgba;

    const gp_Trsf t = (parent * face_loc).Transformation();
    const bool flip = (face.Orientation() == TopAbs_REVERSED) != t.IsNegative();
    const uint32_t base = static_cast<uint32_t>(b.v.size() / 3);
    for (int i = 1; i <= tri->NbNodes(); ++i) {
      gp_Pnt p = tri->Node(i).Transformed(t);
      b.v.push_back(static_cast<float>(p.X()));
      b.v.push_back(static_cast<float>(p.Y()));
      b.v.push_back(static_cast<float>(p.Z()));
    }
    for (int i = 1; i <= tri->NbTriangles(); ++i) {
      int n1, n2, n3;
      tri->Triangle(i).Get(n1, n2, n3);
      if (flip) std::swap(n2, n3);
      b.idx.push_back(base + static_cast<uint32_t>(n1 - 1));
      b.idx.push_back(base + static_cast<uint32_t>(n2 - 1));
      b.idx.push_back(base + static_cast<uint32_t>(n3 - 1));
    }
  }
  return buckets;
}

// Le immagini degli assi x, y, z e la traslazione di una posizione (12 valori).
void matrix_of(const gp_Trsf& t, float* m) {
  for (int j = 1; j <= 3; ++j)
    for (int i = 1; i <= 3; ++i) m[(3 * (j - 1)) + (i - 1)] = static_cast<float>(t.Value(i, j));
  const gp_XYZ p = t.TranslationPart();
  m[9] = static_cast<float>(p.X());
  m[10] = static_cast<float>(p.Y());
  m[11] = static_cast<float>(p.Z());
}

// Un pezzo: le sue parti nella posizione finale; oppure, se compare piu' volte e le istanze sono chieste, la definizione (la
// prima volta) e un'istanza in questa posizione.
void emit(Ctx& c, const TDF_Label& label, const TopLoc_Location& parent, const std::string& name) {
  const std::string entry = entry_of(label);
  if (c.instances && c.uses[entry] > 1) {
    auto it = c.defs.find(entry);
    if (it == c.defs.end()) {
      std::string def_name = label_name(label);
      if (def_name.empty()) def_name = "Parte";
      std::vector<uint32_t> ids;
      for (auto& kv : collect(c, label, TopLoc_Location())) {
        Bucket& b = kv.second;
        if (b.idx.empty()) continue;
        const uint32_t id = c.next_id++;
        ids.push_back(id);
        c.cb(c.user, 1, id, def_name.c_str(), b.rgba.data(), b.v.size() / 3, b.v.data(), b.idx.size() / 3, b.idx.data(), nullptr);
      }
      it = c.defs.emplace(entry, ids).first;
    }
    float m[12];
    matrix_of(parent.Transformation(), m);
    for (uint32_t id : it->second) c.cb(c.user, 2, id, name.c_str(), nullptr, 0, nullptr, 0, nullptr, m);
    return;
  }
  for (auto& kv : collect(c, label, parent)) {
    Bucket& b = kv.second;
    if (b.idx.empty()) continue;
    c.cb(c.user, 0, 0, name.c_str(), b.rgba.data(), b.v.size() / 3, b.v.data(), b.idx.size() / 3, b.idx.data(), nullptr);
  }
}

// Prima passata: quante volte compare ogni pezzo (per le istanze e per contare i triangoli).
void count(Ctx& c, const TDF_Label& label, int depth) {
  if (depth > 64) return;
  if (XCAFDoc_ShapeTool::IsReference(label)) {
    TDF_Label ref;
    if (XCAFDoc_ShapeTool::GetReferredShape(label, ref)) count(c, ref, depth + 1);
    return;
  }
  if (XCAFDoc_ShapeTool::IsAssembly(label)) {
    TDF_LabelSequence comps;
    XCAFDoc_ShapeTool::GetComponents(label, comps);
    for (Standard_Integer i = 1; i <= comps.Length(); ++i) count(c, comps.Value(i), depth + 1);
  } else if (XCAFDoc_ShapeTool::IsSimpleShape(label)) {
    const std::string entry = entry_of(label);
    if (c.uses[entry]++ == 0) {
      c.unique_entries.push_back(entry);
      c.unique_shapes.push_back(XCAFDoc_ShapeTool::GetShape(label));
    }
  }
}

uint64_t triangles_of(const TopoDS_Shape& shape) {
  uint64_t n = 0;
  for (TopExp_Explorer ex(shape, TopAbs_FACE); ex.More(); ex.Next()) {
    TopLoc_Location loc;
    Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(TopoDS::Face(ex.Current()), loc);
    if (!tri.IsNull()) n += static_cast<uint64_t>(tri->NbTriangles());
  }
  return n;
}

// Triangola ogni pezzo una volta e conta i triangoli che arriveranno a SketchUp: con le istanze un pezzo ripetuto si
// costruisce una volta sola.
uint64_t mesh_all(Ctx& c) {
  uint64_t total = 0;
  for (size_t i = 0; i < c.unique_shapes.size(); ++i) {
    const TopoDS_Shape& s = c.unique_shapes[i];
    if (s.IsNull()) continue;
    BRepMesh_IncrementalMesh mesher(s, c.lin, Standard_False, c.ang, Standard_True);
    const uint64_t n = triangles_of(s);
    total += c.instances ? n : n * static_cast<uint64_t>(c.uses[c.unique_entries[i]]);
  }
  return total;
}

// Oltre il tetto: la tolleranza si allarga in proporzione ai triangoli in piu' (piu' o meno inversa al loro numero) e si
// triangola di nuovo, al massimo quattro volte.
void fit_to(Ctx& c, uint64_t max_triangles) {
  uint64_t total = mesh_all(c);
  if (max_triangles == 0) return;
  for (int round = 0; round < 4 && static_cast<double>(total) > static_cast<double>(max_triangles) * 1.05; ++round) {
    const double f = static_cast<double>(total) / static_cast<double>(max_triangles);
    c.lin *= f * 1.15;
    c.ang = std::min(c.ang * std::sqrt(f), 1.2);
    for (const TopoDS_Shape& s : c.unique_shapes)
      if (!s.IsNull()) BRepTools::Clean(s);
    total = mesh_all(c);
  }
}

// OpenCascade triangola in parallelo: si lascia libero un processore, cosi' il computer resta usabile.
void limit_threads() {
  static bool done = false;
  if (done) return;
  done = true;
  const int n = OSD_Parallel::NbLogicalProcessors();
  OSD_ThreadPool::DefaultPool(n > 2 ? n - 1 : 1);
}

void visit(Ctx& c, const TDF_Label& label, const TopLoc_Location& parent, const std::string& path, int depth) {
  if (depth > 64) return;  // un assieme che contiene se' stesso
  if (XCAFDoc_ShapeTool::IsReference(label)) {
    TDF_Label ref;
    if (!XCAFDoc_ShapeTool::GetReferredShape(label, ref)) return;
    visit(c, ref, parent * XCAFDoc_ShapeTool::GetLocation(label), path, depth + 1);
    return;
  }
  std::string n = label_name(label);
  if (n.empty()) n = "Parte";
  const std::string full = path.empty() ? n : path + "/" + n;
  if (XCAFDoc_ShapeTool::IsAssembly(label)) {
    TDF_LabelSequence comps;
    XCAFDoc_ShapeTool::GetComponents(label, comps);
    for (Standard_Integer i = 1; i <= comps.Length(); ++i) visit(c, comps.Value(i), parent, full, depth + 1);
  } else if (XCAFDoc_ShapeTool::IsSimpleShape(label)) {
    const int k = ++c.seen[full];
    emit(c, label, parent, k == 1 ? full : full + " (" + std::to_string(k) + ")");
  }
}

Handle(TDocStd_Document) new_doc() {
  Handle(XCAFApp_Application) app = XCAFApp_Application::GetApplication();
  Handle(TDocStd_Document) doc;
  app->NewDocument(TCollection_ExtendedString("MDTV-XCAF"), doc);
  return doc;
}

}  // namespace

extern "C" {

const char* rstp_occt_version(void) { return OCC_VERSION_STRING_EXT; }

int rstp_load(const char* path, int format, double lin, double ang, uint64_t max_triangles, int instances, rstp_item_cb cb,
              void* user, double* used_linear, char* err, uint32_t err_len) {
  try {
    limit_threads();
    Interface_Static::SetCVal("xstep.cascade.unit", "MM");
    Handle(TDocStd_Document) doc = new_doc();
    if (format == 1) {
      IGESCAFControl_Reader r;
      r.SetColorMode(Standard_True);
      r.SetNameMode(Standard_True);
      if (r.ReadFile(path) != IFSelect_RetDone) {
        set_err(err, err_len, "il file IGES non si apre o non e' valido");
        return 2;
      }
      if (!r.Transfer(doc)) {
        set_err(err, err_len, "il file IGES non contiene forme che si possano convertire");
        return 3;
      }
    } else {
      STEPCAFControl_Reader r;
      r.SetColorMode(Standard_True);
      r.SetNameMode(Standard_True);
      r.SetLayerMode(Standard_False);
      if (r.ReadFile(path) != IFSelect_RetDone) {
        set_err(err, err_len, "il file STEP non si apre o non e' valido");
        return 2;
      }
      if (!r.Transfer(doc)) {
        set_err(err, err_len, "il file STEP non contiene forme che si possano convertire");
        return 3;
      }
    }
    Ctx c;
    c.st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
    c.ct = XCAFDoc_DocumentTool::ColorTool(doc->Main());
    c.lin = lin;
    c.ang = ang;
    c.cb = cb;
    c.user = user;
    c.instances = instances != 0;
    TDF_LabelSequence roots;
    c.st->GetFreeShapes(roots);
    if (roots.Length() == 0) {
      set_err(err, err_len, "il file non contiene forme");
      return 4;
    }
    for (Standard_Integer i = 1; i <= roots.Length(); ++i) count(c, roots.Value(i), 0);
    fit_to(c, max_triangles);
    if (used_linear) *used_linear = c.lin;
    for (Standard_Integer i = 1; i <= roots.Length(); ++i) visit(c, roots.Value(i), TopLoc_Location(), std::string(), 0);
    return 0;
  } catch (const Standard_Failure& e) {
    set_err(err, err_len, std::string("OpenCascade: ") + (e.GetMessageString() ? e.GetMessageString() : e.DynamicType()->Name()));
    return 10;
  } catch (const std::exception& e) {
    set_err(err, err_len, std::string("errore: ") + e.what());
    return 11;
  } catch (...) {
    set_err(err, err_len, "errore sconosciuto in OpenCascade");
    return 12;
  }
}

int rstp_write_test_box(const char* path, int format, int repeated, char* err, uint32_t err_len) {
  try {
    Handle(TDocStd_Document) doc = new_doc();
    Handle(XCAFDoc_ShapeTool) st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
    Handle(XCAFDoc_ColorTool) ct = XCAFDoc_DocumentTool::ColorTool(doc->Main());

    TDF_Label red = st->AddShape(BRepPrimAPI_MakeBox(20.0, 10.0, 5.0).Shape(), Standard_False);
    TDF_Label blue = st->AddShape(BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape(), Standard_False);
    TDataStd_Name::Set(red, TCollection_ExtendedString("CuboRosso"));
    TDataStd_Name::Set(blue, TCollection_ExtendedString("CuboBlu"));
    ct->SetColor(red, Quantity_Color(1.0, 0.0, 0.0, Quantity_TOC_sRGB), XCAFDoc_ColorSurf);
    ct->SetColor(blue, Quantity_Color(0.0, 0.0, 1.0, Quantity_TOC_sRGB), XCAFDoc_ColorSurf);

    TDF_Label assembly = st->NewShape();
    TDataStd_Name::Set(assembly, TCollection_ExtendedString("Assieme"));
    gp_Trsf move;
    move.SetTranslation(gp_Vec(30.0, 0.0, 0.0));
    st->AddComponent(assembly, red, TopLoc_Location());
    st->AddComponent(assembly, blue, TopLoc_Location(move));
    if (repeated) {
      gp_Trsf up;
      up.SetTranslation(gp_Vec(0.0, 0.0, 50.0));
      st->AddComponent(assembly, red, TopLoc_Location(up));
    }
    st->UpdateAssemblies();

    if (format == 1) {
      IGESCAFControl_Writer w;
      w.SetColorMode(Standard_True);
      w.SetNameMode(Standard_True);
      if (!w.Transfer(doc)) {
        set_err(err, err_len, "scrittura IGES di prova non riuscita");
        return 2;
      }
      if (!w.Write(path)) {
        set_err(err, err_len, "impossibile scrivere il file IGES di prova");
        return 3;
      }
    } else {
      STEPCAFControl_Writer w;
      w.SetColorMode(Standard_True);
      w.SetNameMode(Standard_True);
      if (!w.Transfer(doc, STEPControl_AsIs)) {
        set_err(err, err_len, "scrittura STEP di prova non riuscita");
        return 2;
      }
      if (w.Write(path) != IFSelect_RetDone) {
        set_err(err, err_len, "impossibile scrivere il file STEP di prova");
        return 3;
      }
    }
    return 0;
  } catch (const Standard_Failure& e) {
    set_err(err, err_len, std::string("OpenCascade: ") + (e.GetMessageString() ? e.GetMessageString() : e.DynamicType()->Name()));
    return 10;
  } catch (...) {
    set_err(err, err_len, "errore sconosciuto in OpenCascade");
    return 12;
  }
}

}  // extern "C"
