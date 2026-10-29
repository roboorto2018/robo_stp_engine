// Ponte verso Assimp per i file FBX, glTF/GLB, OBJ, 3MF e PLY: legge la scena, percorre i nodi, porta ogni mesh in millimetri e
// negli assi di SketchUp (Z in alto) e la consegna a Rust con la funzione di ritorno. Vedi assimp_bridge.h.

#include "assimp_bridge.h"

#include <assimp/Importer.hpp>
#include <assimp/config.h>
#include <assimp/material.h>
#include <assimp/postprocess.h>
#include <assimp/scene.h>
#include <assimp/version.h>

#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <map>
#include <string>
#include <vector>

namespace fs = std::filesystem;

namespace {

void scrivi_errore(char* err, uint32_t len, const std::string& m) {
    if (!err || len == 0) return;
    size_t n = m.size() < len - 1 ? m.size() : len - 1;
    std::memcpy(err, m.data(), n);
    err[n] = 0;
}

// Un numero dei metadati della scena (gli FBX li scrivono come interi o come numeri con la virgola).
bool numero(const aiMetadata* md, const char* chiave, double& out) {
    if (!md) return false;
    for (unsigned i = 0; i < md->mNumProperties; ++i) {
        if (std::strcmp(md->mKeys[i].C_Str(), chiave) != 0) continue;
        const aiMetadataEntry& e = md->mValues[i];
        switch (e.mType) {
            case AI_BOOL: out = *static_cast<bool*>(e.mData) ? 1.0 : 0.0; return true;
            case AI_INT32: out = *static_cast<int32_t*>(e.mData); return true;
            case AI_UINT32: out = *static_cast<uint32_t*>(e.mData); return true;
            case AI_INT64: out = static_cast<double>(*static_cast<int64_t*>(e.mData)); return true;
            case AI_UINT64: out = static_cast<double>(*static_cast<uint64_t*>(e.mData)); return true;
            case AI_FLOAT: out = *static_cast<float*>(e.mData); return true;
            case AI_DOUBLE: out = *static_cast<double*>(e.mData); return true;
            default: return false;
        }
    }
    return false;
}

// Il vettore unitario di un asse del file: indice 0, 1, 2 e segno.
aiVector3D asse(double indice, double segno) {
    aiVector3D v(0, 0, 0);
    int i = static_cast<int>(indice);
    float s = segno < 0 ? -1.0f : 1.0f;
    if (i == 0) v.x = s; else if (i == 1) v.y = s; else v.z = s;
    return v;
}

// Dagli assi del file a quelli di SketchUp. Prima il sistema "Y in alto" (X a destra, Y in alto, Z verso chi guarda: le righe sono
// gli assi del file), poi da Y in alto a Z in alto: (x, y, z) -> (x, -z, y).
aiMatrix4x4 assi_di_sketchup(const aiMetadata* md, int& alto) {
    double up = 1, up_s = 1, front = 2, front_s = 1, coord = 0, coord_s = 1;
    alto = numero(md, "UpAxis", up) ? static_cast<int>(up) : -1;
    numero(md, "UpAxisSign", up_s);
    numero(md, "FrontAxis", front);
    numero(md, "FrontAxisSign", front_s);
    numero(md, "CoordAxis", coord);
    numero(md, "CoordAxisSign", coord_s);
    aiVector3D destra = asse(coord, coord_s), su = asse(up, up_s), avanti = asse(front, front_s);
    aiMatrix4x4 m(destra.x, destra.y, destra.z, 0, su.x, su.y, su.z, 0, avanti.x, avanti.y, avanti.z, 0, 0, 0, 0, 1);
    aiMatrix4x4 z_su(1, 0, 0, 0, 0, 0, -1, 0, 0, 1, 0, 0, 0, 0, 0, 1);
    return z_su * m;
}

double determinante(const aiMatrix4x4& m) {
    return m.a1 * (m.b2 * m.c3 - m.b3 * m.c2) - m.a2 * (m.b1 * m.c3 - m.b3 * m.c1) + m.a3 * (m.b1 * m.c2 - m.b2 * m.c1);
}

std::string nome_pulito(std::string s) {
    for (char& c : s) {
        if (c == '/' || c == '\\') c = '_'; // la barra separa i gruppi nel plugin
    }
    // Assimp aggiunge ai nodi creati dai pivot nomi come "Box_$AssimpFbx$_Translation"
    size_t k = s.find("_$AssimpFbx$");
    if (k != std::string::npos) s = s.substr(0, k);
    return s;
}

struct Lettore {
    const aiScene* scena = nullptr;
    fs::path cartella;           // dove sta il file FBX
    fs::path cartella_texture;   // dove si scrivono le immagini incorporate
    double mm = 10.0;            // millimetri per unita' del file
    bool uv = false;
    bool istanze = false;
    aiMatrix4x4 radice;          // assi del file -> assi di SketchUp
    rasm_item_cb cb = nullptr;
    void* user = nullptr;
    std::vector<unsigned> usi;   // quante volte ogni mesh compare nei nodi
    std::map<std::string, std::string> texture; // percorso nel file -> percorso trovato
    double triangoli = 0, nodi = 0, incorporate = 0;

    // Il colore del materiale (-1 se non ne ha uno vero) e l'immagine (vuota se non c'e').
    void materiale(unsigned indice, float rgba[4], std::string& immagine) {
        rgba[0] = rgba[1] = rgba[2] = rgba[3] = -1.0f;
        if (indice >= scena->mNumMaterials) return;
        const aiMaterial* m = scena->mMaterials[indice];
        aiString nome;
        m->Get(AI_MATKEY_NAME, nome);
        if (std::strcmp(nome.C_Str(), AI_DEFAULT_MATERIAL_NAME) == 0) return; // il materiale grigio che Assimp aggiunge da se'
        aiColor4D c;
        if (m->Get(AI_MATKEY_BASE_COLOR, c) == AI_SUCCESS || m->Get(AI_MATKEY_COLOR_DIFFUSE, c) == AI_SUCCESS) {
            float opaco = 1.0f;
            if (m->Get(AI_MATKEY_OPACITY, opaco) != AI_SUCCESS || !(opaco > 0.0f)) opaco = 1.0f;
            rgba[0] = c.r; rgba[1] = c.g; rgba[2] = c.b; rgba[3] = opaco > 1.0f ? 1.0f : opaco;
        }
        if (!uv) return;
        aiString p;
        if (m->GetTexture(aiTextureType_BASE_COLOR, 0, &p) != AI_SUCCESS && m->GetTexture(aiTextureType_DIFFUSE, 0, &p) != AI_SUCCESS) return;
        immagine = trova_immagine(p.C_Str());
    }

    // L'immagine incorporata nel file si scrive nella cartella delle texture; un'immagine esterna si cerca dove dice il file, poi
    // accanto al file FBX. Se non si trova resta il percorso del file (il plugin la cerca ancora accanto al file).
    std::string trova_immagine(const std::string& salvata) {
        auto gia = texture.find(salvata);
        if (gia != texture.end()) return gia->second;
        std::string trovata = salvata;
        const aiTexture* t = scena->GetEmbeddedTexture(salvata.c_str());
        if (t && t->mHeight == 0 && t->mWidth > 0) {
            std::string tipo = t->achFormatHint[0] ? std::string(t->achFormatHint) : "png";
            std::string base = fs::u8path(salvata).stem().u8string();
            if (base.empty() || base[0] == '*') base = "texture_" + std::to_string(texture.size() + 1);
            std::error_code ec;
            fs::create_directories(cartella_texture, ec);
            fs::path f = cartella_texture / fs::u8path(nome_pulito(base) + "." + tipo);
            std::ofstream out(f, std::ios::binary);
            out.write(reinterpret_cast<const char*>(t->pcData), t->mWidth);
            if (out) {
                trovata = f.u8string();
                incorporate += 1;
            }
        } else if (!salvata.empty() && salvata[0] != '*') {
            fs::path p = fs::u8path(salvata);
            std::error_code ec;
            for (const fs::path& c : {p, cartella / p, cartella / p.filename()}) {
                if (fs::is_regular_file(c, ec)) {
                    trovata = c.u8string();
                    break;
                }
            }
        }
        texture[salvata] = trovata;
        return trovata;
    }

    // Una mesh: in `m` (se non e' nullo) la posizione finale, altrimenti nelle sue coordinate (una definizione).
    void consegna(int tipo, uint32_t id, const std::string& nome, const aiMesh* mesh, const aiMatrix4x4* m) {
        std::vector<float> v;
        std::vector<float> coord;
        std::vector<uint32_t> idx;
        v.reserve(mesh->mNumVertices * 3);
        for (unsigned i = 0; i < mesh->mNumVertices; ++i) {
            aiVector3D p = m ? (*m) * mesh->mVertices[i] : mesh->mVertices[i];
            v.push_back(static_cast<float>(p.x * mm));
            v.push_back(static_cast<float>(p.y * mm));
            v.push_back(static_cast<float>(p.z * mm));
        }
        // una posizione specchiata (determinante negativo) gira i triangoli: si rimettono con la faccia verso l'esterno
        bool gira = m && determinante(*m) < 0;
        for (unsigned f = 0; f < mesh->mNumFaces; ++f) {
            const aiFace& face = mesh->mFaces[f];
            if (face.mNumIndices != 3) continue;
            idx.push_back(face.mIndices[0]);
            idx.push_back(gira ? face.mIndices[2] : face.mIndices[1]);
            idx.push_back(gira ? face.mIndices[1] : face.mIndices[2]);
        }
        if (idx.empty()) return;
        float rgba[4];
        std::string immagine;
        materiale(mesh->mMaterialIndex, rgba, immagine);
        // nessun colore dal materiale ma colori nei vertici (le scansioni PLY): la loro media
        if (rgba[0] < 0 && mesh->HasVertexColors(0) && mesh->mNumVertices > 0) {
            double somma[4] = {0, 0, 0, 0};
            for (unsigned i = 0; i < mesh->mNumVertices; ++i) {
                const aiColor4D& c = mesh->mColors[0][i];
                somma[0] += c.r; somma[1] += c.g; somma[2] += c.b; somma[3] += c.a;
            }
            for (int k = 0; k < 4; ++k) rgba[k] = static_cast<float>(somma[k] / mesh->mNumVertices);
            if (!(rgba[3] > 0.0f)) rgba[3] = 1.0f;
        }
        const float* puv = nullptr;
        if (uv && !immagine.empty() && mesh->HasTextureCoords(0)) {
            coord.reserve(mesh->mNumVertices * 2);
            for (unsigned i = 0; i < mesh->mNumVertices; ++i) {
                coord.push_back(mesh->mTextureCoords[0][i].x);
                coord.push_back(mesh->mTextureCoords[0][i].y);
            }
            puv = coord.data();
        }
        cb(user, tipo, id, nome.c_str(), rgba, mesh->mNumVertices, v.data(), idx.size() / 3, idx.data(), puv,
           puv ? immagine.c_str() : nullptr, nullptr);
    }

    void conta(const aiNode* n) {
        for (unsigned k = 0; k < n->mNumMeshes; ++k) usi[n->mMeshes[k]] += 1;
        for (unsigned c = 0; c < n->mNumChildren; ++c) conta(n->mChildren[c]);
    }

    void definizioni() {
        if (!istanze) return;
        for (unsigned i = 0; i < scena->mNumMeshes; ++i) {
            if (usi[i] > 1) consegna(1, i + 1, nome_pulito(scena->mMeshes[i]->mName.C_Str()), scena->mMeshes[i], nullptr);
        }
    }

    // Percorre i nodi: il nome di una parte e' il percorso dei nodi ("Padre/Figlio"); un nodo con piu' mesh (una per materiale)
    // le mette ognuna in un suo sottogruppo.
    void visita(const aiNode* n, const aiMatrix4x4& sopra, const std::string& percorso, bool radice_scena) {
        aiMatrix4x4 mondo = sopra * n->mTransformation;
        std::string nome = nome_pulito(n->mName.C_Str());
        if (nome.empty()) nome = "Nodo";
        // il nodo radice ("RootNode") non diventa un gruppo; i nodi dei pivot senza mesh si fondono con il loro figlio
        bool pivot = std::strstr(n->mName.C_Str(), "_$AssimpFbx$") != nullptr;
        std::string qui = radice_scena || pivot ? percorso : (percorso.empty() ? nome : percorso + "/" + nome);
        if (n->mNumMeshes > 0) nodi += 1;
        for (unsigned k = 0; k < n->mNumMeshes; ++k) {
            unsigned i = n->mMeshes[k];
            const aiMesh* mesh = scena->mMeshes[i];
            triangoli += mesh->mNumFaces;
            std::string parte = qui.empty() ? nome : qui;
            if (n->mNumMeshes > 1) {
                std::string sotto = nome_pulito(mesh->mName.C_Str());
                aiString mat;
                if (mesh->mMaterialIndex < scena->mNumMaterials) scena->mMaterials[mesh->mMaterialIndex]->Get(AI_MATKEY_NAME, mat);
                if (sotto.empty() || sotto == nome) sotto = nome_pulito(mat.C_Str());
                if (sotto.empty()) sotto = "Mesh";
                parte += "/" + sotto + "_" + std::to_string(k + 1);
            }
            if (istanze && usi[i] > 1) {
                float matrice[12] = {static_cast<float>(mondo.a1), static_cast<float>(mondo.b1), static_cast<float>(mondo.c1),
                                     static_cast<float>(mondo.a2), static_cast<float>(mondo.b2), static_cast<float>(mondo.c2),
                                     static_cast<float>(mondo.a3), static_cast<float>(mondo.b3), static_cast<float>(mondo.c3),
                                     static_cast<float>(mondo.a4 * mm), static_cast<float>(mondo.b4 * mm), static_cast<float>(mondo.c4 * mm)};
                cb(user, 2, i + 1, parte.c_str(), nullptr, 0, nullptr, 0, nullptr, nullptr, nullptr, matrice);
            } else {
                consegna(0, 0, parte, mesh, &mondo);
            }
        }
        for (unsigned c = 0; c < n->mNumChildren; ++c) visita(n->mChildren[c], mondo, qui, false);
    }
};

} // namespace

extern "C" int rasm_load(const char* path_utf8, double unit_mm, double default_unit_mm, int file_axes, int instances, int with_uv,
                         const char* texture_dir_utf8, rasm_item_cb cb, void* user, double* info, char* err, uint32_t err_len) {
    try {
        Assimp::Importer imp;
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_READ_ANIMATIONS, false);
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_READ_CAMERAS, false);
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_READ_LIGHTS, false);
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_PRESERVE_PIVOTS, false); // i pivot entrano nella posizione dei nodi
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_EMBEDDED_TEXTURES_LEGACY_NAMING, false);
        imp.SetPropertyInteger(AI_CONFIG_PP_SBP_REMOVE, aiPrimitiveType_POINT | aiPrimitiveType_LINE);
        // senza normali (e senza coordinate di mappatura, se non servono) i vertici uguali si uniscono: meno punti per SketchUp
        // i colori dei vertici restano: senza un colore del materiale la mesh prende la loro media (scansioni PLY)
        int via = aiComponent_NORMALS | aiComponent_TANGENTS_AND_BITANGENTS | aiComponent_BONEWEIGHTS | aiComponent_ANIMATIONS |
                  aiComponent_LIGHTS | aiComponent_CAMERAS;
        if (!with_uv) via |= aiComponent_TEXCOORDS | aiComponent_TEXTURES;
        imp.SetPropertyInteger(AI_CONFIG_PP_RVC_FLAGS, via);
        const aiScene* scena = imp.ReadFile(path_utf8, aiProcess_RemoveComponent | aiProcess_Triangulate | aiProcess_JoinIdenticalVertices |
                                                          aiProcess_SortByPType);
        if (!scena || !scena->mRootNode) {
            scrivi_errore(err, err_len, std::string("Assimp non legge il file: ") + imp.GetErrorString());
            return 1;
        }
        Lettore l;
        l.scena = scena;
        l.cartella = fs::u8path(path_utf8).parent_path();
        l.cartella_texture = fs::u8path(texture_dir_utf8 ? texture_dir_utf8 : "");
        // l'unita': quella scritta nel file (FBX: UnitScaleFactor, centimetri per unita'), altrimenti quella del formato
        double fattore = 0.0;
        bool dal_file = numero(scena->mMetaData, "UnitScaleFactor", fattore) && fattore > 0;
        double del_file = dal_file ? fattore * 10.0 : (default_unit_mm > 0 ? default_unit_mm : 1.0);
        l.mm = unit_mm > 0 ? unit_mm : del_file;
        l.uv = with_uv != 0;
        l.istanze = instances != 0;
        l.cb = cb;
        l.user = user;
        int alto = -1;
        aiMatrix4x4 assi = assi_di_sketchup(scena->mMetaData, alto);
        l.radice = file_axes ? assi : aiMatrix4x4();
        l.usi.assign(scena->mNumMeshes, 0);
        l.conta(scena->mRootNode);
        l.definizioni();
        l.visita(scena->mRootNode, l.radice, "", true);
        if (info) {
            info[0] = del_file;
            info[1] = alto;
            info[2] = scena->mNumMeshes;
            info[3] = l.nodi;
            info[4] = l.triangoli;
            info[5] = l.incorporate;
            info[6] = dal_file ? 1 : 0;
        }
        return 0;
    } catch (const std::exception& e) {
        scrivi_errore(err, err_len, std::string("errore nella lettura del file: ") + e.what());
        return 2;
    } catch (...) {
        scrivi_errore(err, err_len, "errore sconosciuto nella lettura del file");
        return 3;
    }
}

extern "C" const char* rasm_assimp_version(void) {
    static std::string v = std::to_string(aiGetVersionMajor()) + "." + std::to_string(aiGetVersionMinor()) + "." + std::to_string(aiGetVersionPatch());
    return v.c_str();
}
