' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Ann
' corpus-prints: 31
Private Type Person
    Name As String
    Age As Integer
End Type
Sub Main()
    Dim p As Person
    p.Name = "Ann"
    p.Age = 30
    p.Age = p.Age + 1
    Debug.Print p.Name
    Debug.Print p.Age
End Sub
